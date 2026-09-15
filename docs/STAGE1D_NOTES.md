# Stage 1 (d) — annotation tools, properties, forms overlay, undo/redo

Module (d) of `WORKPLAN.md` §3: the tool state machines, the SVG annotation overlay, the HTML form
overlay, the properties panel, the 주석 sidebar tab and the undo/redo reconciliation. Companions:
`ARCHITECTURE.md` §6.1/§7/§10, `IPC_CONTRACT.md` §7.1/§7.2/§7.8, `UI_SPEC.md` §3/§6/§7/§12,
`FEATURES.md` F-08…F-15, F-20, and `STAGE1C_NOTES.md` (the frozen `PageShell` slots this builds on).

```
src/tools/          ToolController.ts  commands.ts  registry.ts   ← eager seam + lazy registration
                    geometry.ts hit.ts                            ← pure maths, no React, no IPC
                    markup.ts ink.ts shapes.ts line.ts note.ts textbox.ts stamp.ts eraser.ts select.ts
src/annot/          bridge.tsx          ← the only (d) file on the critical path (≈ 40 lines)
                    AnnotationHost.tsx  ← lazy entry: sink, keyboard, layer renderer
                    AnnotOverlay.tsx shapes.tsx ToolSurface.tsx editors.tsx RenderProbe.tsx
                    actions.ts sync.ts selectionQuads.ts annot.css
src/forms/          FormLayer.tsx formStore.ts
src/app/Inspector/  index.tsx (frame, eager) InspectorBody.tsx apply.ts patch.ts
src/sidebar/        AnnotationList.tsx
src/store/          annotStore.ts
```

---

## 1. The tool contract (what the tool strip and the keymap talk to)

`toolController` keeps its Stage 0 export name and grows a sink, a subscription and a `cancel`.
Every tool is still the pure machine ARCHITECTURE §10 promised:

```ts
interface ToolModule<S> {
  id: ToolId; cursor: string;
  init(ctx): S;
  onDown(state, p: PagePoint, ctx): ToolResult<S>;
  onMove(state, p, ctx): ToolResult<S>;
  onUp(state, p, ctx): ToolResult<S>;
  onKey?(state, key, ctx): ToolResult<S>;
  onArm?(): void;            // 도장/서명 open the picker here
  hoverPreview?: boolean;    // eraser circle / stamp ghost track a hovering pointer
}

interface ToolResult<S> {
  state: S;
  preview?: ToolPreview | null;                 // `undefined` keeps it, `null` clears it
  commit?: { page; spec: AnnotSpec };           // create
  patch?: { page; edits: { id; patch }[]; live? };   // move / resize, `live` coalesces
  erase?: { page; ids };  select?: { ids; additive? };
  edit?: { page; rect?; id? };                  // open an inline editor
  done?: boolean;                               // hand control back to 선택
}
```

`ToolContext` gained four injected capabilities so the machines stay pure and testable:
`scale` (CSS px per pt — every tolerance is `px / scale`), `annots(page)`, `selected`,
`selectionRects(page)` and `image`. The annotation host supplies them from the stores; a test
supplies fakes (`src/tools/tools.test.ts`).

`ToolResult.commit` carries the page as well as the spec (Stage 0's stub returned a bare
`AnnotSpec`, which has no page). That is the only shape change to a Stage 0 type.

Registration is deliberately **not** in `ToolController.ts`: `src/tools/registry.ts` does it and is
reached by a dynamic import, so the tool strip's eager import of the controller costs ~2 kB.

| Tool | File | Gesture | Produces |
|---|---|---|---|
| 형광펜/밑줄/취소선/물결선 | `markup.ts` | the viewer's text drag | `rects` = one rect per line run |
| 펜 | `ink.ts` | freehand, ⇧ = straight | one `/InkList` stroke, RDP-simplified + 3-tap smoothed |
| 지우개 | `eraser.ts` | drag | `delete_annotations` of the whole strokes it touched, one call |
| 사각형/타원 | `shapes.ts` | drag, ⇧ square, ⌥ from centre | `square` / `circle` |
| 선/화살표 | `line.ts` | drag, ⇧ 15° snap | `line` / `arrow` (engine writes Ink + `/Subj`) |
| 텍스트 상자 | `textbox.ts` | drag or click | **no spec** — an `edit` rect; the editor creates it with its text |
| 메모 | `note.ts` | click | `note` with empty contents, popover opens focused |
| 도장/서명 | `stamp.ts` | arm → picker, click/drag | `stamp` with `{ path }` |
| 선택 | `select.ts` | click/drag/handles/marquee | `select` + `patch` |

---

## 2. Two lists, and why an annotation is not painted twice

`annotStore` holds `byPage` (what `list_annotations` returned) and `ghosts` (optimistic).

* **`byPage` annotations are already inside the page bitmap** — pdfium renders them. The overlay
  therefore draws them as *nothing*, plus an outline when hovered or selected, plus handles when
  selected. Painting them in SVG as well would double every highlight's opacity.
* **Ghosts are painted in full**, because no bitmap contains them yet.

A ghost is dropped by `annotStore.pageRendered(page, generation)` once the page has been re-rendered
at a generation that contains it. Dropping it earlier flashes the annotation off for a frame; never
dropping it double-draws. See §7 integration request 1 for how that signal is obtained today.

Hit-testing reads `annotsOnPage(page)` (`actions.ts`), which merges the two so a freshly drawn shape
is selectable before its command resolves.

---

## 3. Layers, and the one CSS rule that is not (d)'s own

`renderLayers(ctx)` fills all four `PageShell` slots on every mounted page:

| slot | content | mounted when |
|---|---|---|
| `under` | `<RenderProbe>` | always (renders nothing unless a ghost is pending) |
| `annotations` | `<AnnotOverlay>` — outlines, ghosts, preview, handles | always |
| `forms` | `<FormLayer>` — one HTML control per widget | 양식 mode, page has fields |
| `surface` | `<ToolSurface>` + 메모 popover + 텍스트 상자 editor | 주석 mode with a drawing/선택 tool, or an editor is open |

`annot.css` adds `pointer-events: none` to `.page-surface` / `.page-forms` and `auto` to their
children. Without it an always-mounted empty surface would swallow the drag that makes a text
selection. That is a rule about (c)'s elements written in (d)'s stylesheet — flagged here so the
integrator can move it into `viewer.css` if (c) prefers.

**The markup tools get no surface on purpose.** The viewer's own pointer handler must keep producing
the text selection they read (`Scroller.selectsText` already lists all four), so the markup commit
hangs off a window `pointerup` in `AnnotationHost` instead.

Overlay chrome that sits on the page uses page colours, not app-theme colours: `.viewer .page-shell`
defines `--page-ink` and `--page-field` next to (c)'s `var(--page-paper, #fff)`. Without that,
handles are black squares and form text is light grey on white paper in dark mode (both were
reproduced and fixed during the live pass).

---

## 4. Undo / redo, and coalescing

* Dragging a slider or an annotation produces a patch per pointer move. `patchAnnotation(…, live)`
  merges them per (page, id) and sends one `update_annotation` after `PATCH_COALESCE_MS` (140 ms) of
  idle, or immediately when the gesture ends. One gesture = one command = **one undo step**.
* `edit.undo` / `edit.redo` in `useCommands` now go through `annot/sync.ts`'s `undoWithAnnots` /
  `redoWithAnnots`, which `flushPatches()` first — otherwise the engine would pop a snapshot that
  does not contain the edit the user is still looking at.
* `doc-changed` → `applyDocChanged` re-lists the changed pages (`'all'` re-lists only pages already
  known, so one ⌘Z on a 500-page file is not 500 commands). `annotStore.setPage` drops a reply that
  belongs to an older generation, which is what stops a slow `list_annotations` from resurrecting an
  undone annotation.
* **`canUndo`/`canRedo` for the title bar**: `docStore.applyDocChanged` carries only the generation
  and the dirty flag, and the title bar reads `info.canUndo`. So `applyDocChanged` in `sync.ts`
  pulls a fresh `DocInfo` (`docStore.refresh()`) on every non-structural change. That is one extra
  metadata read per edit; the alternative is putting `canUndo` into `DocChangedEvent`, which is a
  contract change (§7, request 4).

---

## 5. Forms (F-20)

`list_form_fields` once per (document, generation) while 양식 mode is active — a reader never pays
for it. One real DOM control per widget, placed with `ctx.rectToBox`.

**Korean input is the reason the text fields are uncontrolled.** `defaultValue` + commit on blur /
Enter; a controlled `value` re-renders mid-composition and drops the jamo being composed. Checkboxes,
radios and selects commit on change because they have no composition. The engine's reply replaces
the local copy, so a comb field that truncates or a radio group that clears its siblings is shown as
the file actually has it, never as the optimistic string.

필드 강조 표시 is wired in the tool strip to `formStore.highlight`, which tints the overlay controls.
The spec puts this in the tile URL's `hl=1` — see §7 request 2.

---

## 6. Bundle budget

The critical path was at 116.6 kB gz of 120 when (d) started. Everything (d) adds is code-split:

| chunk | gz | contains |
|---|---|---|
| `AnnotationHost` | 9.1 kB | overlay, tools, surface, editors, sync, forms |
| `actions` | 4.2 kB | the IPC + optimism layer (shared with the inspector and the sidebar) |
| `InspectorBody` | 2.6 kB | the properties panel body |
| `AnnotationList` | 1.8 kB | the 주석 tab |

On the critical path (d) leaves only `ToolController.ts`, `tools/commands.ts`, `annot/bridge.tsx`
and the Inspector frame — together under 3 kB gz. Measured after this module: **103.8 kB gz**
(entry JS 93.9 + CSS 9.8), `node scripts/check-bundle-size.mjs` passes.

`bridge.tsx` deliberately does **not** use `React.lazy`: a `Suspense` fallback would change the
element type at the `<Viewer>` position and remount the scroller — throwing away its scroll offset,
its tile inventory and the text-layer cache — the moment the chunk lands. Instead the tree shape is
constant and only the `layers` callback changes.

---

## 7. Integration requests and gaps

1. **`PageShell` does not expose the tile `onload` to a layer.** ARCHITECTURE §10 says the ghost is
   removed on the new tile's `onload`, and `PageShell` has that callback — but only for its own
   `TileManager`. Until `PageLayerContext` carries an `onPageRendered(cb)` (or `PageShell` calls
   `layers.onTileLoad`), `src/annot/RenderProbe.tsx` asks the question itself: it requests the page's
   placeholder bitmap at the generation that contains the annotation — byte-identical to the URL
   `PageShell` is already loading, so it is a webview cache hit — and settles the ghost on its
   `onload`. Deleting `RenderProbe.tsx` and calling `annotStore.pageRendered(page, gen)` from
   `PageShell`'s `markLoaded` is a ~5-line change for (c)/the integrator.
2. **The viewer never passes `hl` to `tileUrl` / `pageUrl`**, so 필드 강조 표시 cannot tint the
   rendered widget (IPC_CONTRACT §9, UI_SPEC §3). `protocol.ts` already supports the parameter;
   the Scroller has to thread a flag through. (d)'s overlay tint is the stand-in and is worth
   keeping anyway — it makes an *empty* field discoverable at any zoom.
3. **`--page-paper` and `--page-ink` are not tokens.** `.page-shell` already uses
   `var(--page-paper, #fff)` (STAGE1C_NOTES §8) and (d) adds `--page-ink` / `--page-field` locally.
   All three belong in `styles/tokens.css`, defined once and *not* redefined per theme — they are
   the colours of paper, not of chrome.
4. **`DocChangedEvent` has no `canUndo`/`canRedo`.** See §4; today (d) pays a `get_document` per
   edit to keep the title bar's ⌘Z button honest.
5. **서명 만들기 does not exist.** UI_SPEC §6 wants a draw/type/image sheet on first use of 서명;
   nobody owns it (it is not in (e)'s dialog list). (d) ships the image path only: arming 도장 or
   서명 opens the file picker, and a drawn signature would be an `ink`/`signature` spec from the
   same sheet. `setStampPicker` in `src/tools/stamp.ts` is the seam to hang it on.
6. **A moved annotation shows its old position until the page re-renders**, because pdfium's bitmap
   still has it there and the overlay only draws the outline at the new place. `set_annotations_hidden`
   (P1, IPC_CONTRACT §7.1) is the fix: hide the original for the duration of the drag.
7. **The mock's page bitmaps do not draw annotations**, so in `VITE_SEEPDF_MOCK=1` a created
   annotation is visible only as its ghost (one frame) and then as its selection outline. Everything
   below in §8 was verified that way. Teaching `src/ipc/mock.ts`'s PNG generator to paint the
   annotation rectangles would make the mock a much better demo; it is Stage 0's file.
8. **`edit.copy` / `edit.cut` / `edit.paste` on macOS never reach `useCommands`** — they are
   predefined native menu items with no event (STAGE1C_NOTES §7.2). The annotation clipboard is
   implemented and reachable from the command bus, but on macOS only through a future context menu.
9. **Momentary (hold) tools** are routed (`toolController.arm(tool, true)`), but only `Space` = 손 is
   declared momentary in the keymap; if the spec wants held `H`/`P` too, that is a keymap change.
10. **No context menus yet** (UI_SPEC §12). (e) owns `ContextMenu.tsx`; the annotation entries
    (편집 · 속성… · 메모 열기 · 복사 · 삭제 · 이 스타일을 기본값으로) map 1:1 onto
    `runAnnotCommand` + `annotStore.setEditing`.

### Files outside `src/tools|annot|forms|app/Inspector|sidebar|store` that (d) touched

1. `src/app/CanvasStub.tsx` — one line: re-exports `AnnotatedCanvas` from `src/annot/bridge` instead
   of the bare `Viewer`, which is what makes the layer slots reachable. `App.tsx` is untouched.
2. `src/app/Inspector.tsx` — **deleted**, replaced by `src/app/Inspector/index.tsx` (the directory
   WORKPLAN assigns to (d)). The import site `./app/Inspector` resolves to the directory, so no
   caller changed.
3. `src/app/SidebarFrame.tsx` — the 주석 tab's empty state replaced by a lazily imported
   `<AnnotationList />`.
4. `src/app/ToolStrip.tsx` — 필드 강조 표시 (toggle) and 모든 필드 지우기 (action) wired to
   `formStore`; the tool buttons are unchanged.
5. `src/app/useCommands.ts` — `edit.undo`/`edit.redo` route through `annot/sync`, and
   `edit.delete/duplicate/copy/cut/paste` ask `runAnnotCommand` first (it answers `false` when the
   annotation chunk is not loaded or nothing is selected, and the existing 페이지-mode branches run
   unchanged).

---

## 8. Verified live (`VITE_SEEPDF_MOCK=1`, Chrome, port 1420, dark **and** light theme)

Screenshot-checked, zero console errors or warnings (only Vite HMR and React-DevTools notices):

* 형광펜 over a text drag → the annotation appears in the 주석 tab and its selection outline covers
  exactly the line run it was dragged over (quads are correct, one per line).
* 사각형 drag → created, auto-selected, 8 handles; 선택 tool drag moves it by exactly the pointer
  delta; a corner handle resizes it; ⌘Z undoes the resize and ⇧⌘Z redoes it.
* Inspector: the red swatch repaints the selected annotation (the 주석 list's type icon follows) and
  the 불투명도 slider drags without a command per frame.
* 펜, 선, 화살표 created; 지우개 dragged across them removes exactly the strokes it touched, in one
  step; 화살표 outside the dab survives.
* 메모 click → popover opens focused, "한글 메모 테스트" typed and saved on blur; 텍스트 상자 drag →
  inline editor on paper-white with the annotation's colour, "한글 테스트" committed on blur.
* Marquee over two annotations → "주석 2개" in the panel, ⌫ deletes both.
* 양식 mode: three fields tinted by 필드 강조 표시, "박현우" typed into `applicant.name` and committed
  on Tab (dark ink on paper), the eye toggle turns the tint off, the panel shows 필드 이름 / 유형 /
  필수 항목 / 값 지우기.
* Light theme: list, filter chips, popover and inspector all legible.

Fixed *because of* that pass: black handles in dark mode, light-grey form text on white paper, a
collapsed swatch row in the 메모 popover, 굵기 offered for markup and notes (UI_SPEC §7 does not),
and invisible form fields with no highlight.

Not exercised live: 도장 / 서명 placement (needs the Tauri dialog plugin — the browser mock has no
file picker) and anything that depends on the engine actually rendering an annotation (§7.7).

## 9. Tests

```sh
npx vitest run src/tools src/annot src/forms src/app/Inspector src/sidebar/AnnotationList
```

* `tools.test.ts` — 30 cases, one per state machine: markup quads (including "one rect per line" and
  the whitespace filter), ink simplification + ⇧, shape ⇧/⌥/click, line 15° snap + heads, note,
  textbox `edit` rect, stamp placement, eraser hit set, select click/move/resize/marquee/locked,
  plus the controller's routing and `cancel`.
* `annot/selectionQuads.test.ts` — drives the **real** text layer of the mock document and asserts
  the rectangles the markup tool commits.
* `annot/actions.test.ts` — ghost before the command resolves, ghost removed on failure, coalescing
  (20 slider steps → 1 command, merged payload, idle flush), delete, duplicate offset, spec round-trip.
* `annot/sync.test.ts` — undo drops the annotation and redo brings it back at a new generation, a
  stale reply is ignored, another document is ignored, patches flush before the snapshot is popped,
  ghost lifetime per page/generation.
* `annot/overlay.test.tsx` — the ghost disappears on the page bitmap's `onload`, no probe when there
  is nothing pending, a listed annotation is outlined rather than repainted, handles stay 14 CSS px
  at 400 %.
* `app/Inspector/patch.test.ts` — every control → its `AnnotPatch` field (굵기 → `borderWidth`),
  the controls with no wire field, coalescing flags, and the selection-vs-default rule.
* `forms/forms.test.tsx` — field list, commit keeps the engine's answer, stale reply dropped,
  controls placed by the page matrix, a Hangul composition reaches the engine once on blur,
  checkbox commits immediately, nothing renders outside 양식 mode.
* `sidebar/AnnotationList.test.tsx` — grouping, ghost merge, kind filter, click → select + scroll +
  popover, empty state.
