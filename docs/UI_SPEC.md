# SeePDF — UI specification (v1)

Source: `docs/spikes/ux-research.md` §4–§6, narrowed to what v1 ships (`FEATURES.md`) and to what the
engine can actually do (`ARCHITECTURE.md`). Korean is the design language: lay out and size every
component against the Korean string first, then check English.

---

## 1. Window and layout

Single-window, **mode-switcher** shell (PDF Expert's model — not a ribbon, not an icon rail).
One document per window; the first window shows the welcome screen.

```
┌──────────────────────────────────────────────────────────────────────────────────────┐
│ ⌃ traffic lights │ ☰  ↶ ↷   report-2026.pdf ⌄   [읽기][주석][편집][페이지][양식]  🔍 ⤓ ⋯ │ 44px
├───────────┬──────────────────────────────────────────────────────┬───────────────────┤
│           │ ┌──── contextual tool strip (주석/편집/양식 only) ───┐ │                   │ 40px
│ SIDEBAR   │ └────────────────────────────────────────────────────┘ │  PROPERTIES       │
│ 240px     │                   ┌───────────────┐                    │  260px            │
│ ▤ ≡ 💬 🔍 │                   │    PAGE 1     │                    │  (collapsible)    │
│           │                   └───────────────┘                    │                   │
├───────────┴──────────────────────────────────────────────────────┴───────────────────┤
│ ◀ 3 / 148 ▶ │ 단일 연속 두 쪽 │ ↺ ↻ │  − ──●── +  118% ⌄ │            저장됨 ✓        │ 28px
└──────────────────────────────────────────────────────────────────────────────────────┘
```

* macOS: `titleBarStyle: "Overlay"` + `hiddenTitle: true`; the top 44 px strip is
  `data-tauri-drag-region` and reserves `padding-left: 78px` under `[data-os="macos"]` for the traffic
  lights (`env(titlebar-area-*)` is not available in WKWebView).
* Windows: native caption bar (v1 decision — less work, fewer platform bugs); the same 44 px toolbar sits
  below it with `padding-left: 8px`.
* Chrome heights are fixed tokens: titlebar 44 · tool strip 40 · status bar 28 · sidebar 240 (drag
  180–420, collapsible to 0) · inspector 260.
* Below a 900 px window width: tool buttons collapse into `⋯`, mode labels become icon-only, and the
  document title truncates first. **The toolbar never wraps.**

---

## 2. Title-bar toolbar (always visible)

| Zone | Contents |
|---|---|
| left gutter | 78 px (macOS) / 8 px (Windows), drag region |
| left group | `☰` sidebar toggle · `↶` undo · `↷` redo (undo/redo only when a document is open) |
| centre-left | document title + `⌄` menu: 경로 복사 / Finder에서 보기 / 문서 정보… ; middle-truncated; a `•` prefix when dirty |
| centre | **mode switcher**: 읽기 · 주석 · 편집 · 페이지 · 양식 (segmented, ⌘1–⌘5) |
| right | `🔍` search toggle · `⤓` export · `⋯` overflow (인쇄, 보안, 워터마크, 압축, 문서 비교, OCR, 여러 파일 OCR, 여러 파일에서 검색, 합치기, 분할, 문서 정보, 설정) |

Changing mode changes four things at once: the tool strip, the default canvas cursor, the properties
panel content, and what a click on the page does.

---

## 3. Contextual tool strip (40 px; 주석 / 편집 / 양식 only)

| Mode | Tools (left to right) |
|---|---|
| 주석 | 선택 · 형광펜 · 밑줄 · 취소선 · 물결선 · 메모 · 펜 · 지우개 · 사각형 · 타원 · 선 · 화살표 · 텍스트 상자 · 도장 · 서명 |
| 편집 | 선택 · 텍스트 수정 · 텍스트 추가 · 이미지 추가 · 영역 표시(redact) |
| 양식 | 선택 · 필드 강조 표시 (toggle) · 모든 필드 지우기 |

Icon-only buttons, 28×28 with a 20 px `lucide-react` glyph, tooltip after 500 ms with the shortcut in a
dimmer weight. The active tool has an `--accent-subtle` background and a 1 px accent border.

---

## 4. Left sidebar (4 tabs, 28 px icon buttons at the top)

| Tab | Icon | Content | Empty state |
|---|---|---|---|
| 축소판 | `rectangle-vertical` | 1–2 column thumbnail list, page number under each, current page with a 2 px accent ring, drag to reorder, ⌘/⇧ multi-select, context menu | — |
| 목차 | `list-tree` | outline tree with disclosure triangles, current section highlighted on scroll | `sidebar.outline.empty` |
| 주석 | `message-square` | flat list grouped by page: type icon, author, excerpt, timestamp; click scrolls and selects; filter chips by type (a thread is filtered by its top-level annotation's type). **Threads (P2):** replies are listed under their top-level annotation — indented, `corner-down-right`, excerpt + 작성자 · 날짜, flattened and oldest first — behind a count badge (`message-square` + N) that folds / unfolds them (open by default); a click on a reply opens the thread's popover; right-click on a row = 답글 · 삭제 | `sidebar.annotations.empty` |
| 검색 | `search` | query field, 대소문자 구분 / 단어 단위 toggles, result count, results with ±40 characters of context and the match in bold, grouped by page, streaming in as they arrive; a quiet 여러 파일에서 검색… button (P2) under the toggles | `sidebar.search.empty` |

Sidebar open/closed, active tab and width persist per app (not per document) in `settings.json`.

---

## 5. Canvas

* Virtualised: only pages within ±1.5 screens are mounted; each mounted page is a `PageShell` with the
  layers of `ARCHITECTURE.md` §10 (placeholder `<img>` → tile `<img>`s → selection/search rects →
  SVG annotation overlay → form inputs → pointer capture surface).
* Page gap 16 px, `--page-shadow`, canvas background `--bg-canvas`.
* Zoom during a gesture is a CSS transform; sharp tiles replace it on settle (120 ms debounce).
  The point under the cursor is the zoom anchor.
* Night mode (P1-10: 끄기 → 어둡게 → 세피아, one step per ⌃⌘N / 보기 ▸ 야간 모드 / the status-bar `moon`)
  filters the tile layer only; the SVG overlay, the selection and search marks and the form inputs are never
  filtered. Night bitmaps are rendered on a transparent clear colour, so the page shell shows the night paper:
  어둡게 = `invert(0.9) hue-rotate(180deg)` over `--night-paper` (#1a, exactly the filtered white), 세피아 =
  `sepia(0.2)` multiplied onto `--sepia-paper`. The `--page-*` tokens are re-scoped to those papers while a
  night mode is on; thumbnails, the organizer and the compare view stay day-rendered. Persisted in
  `Settings.night` (Stage 8): every change is saved and each window starts in the remembered mode.
* **분할 보기** (P2; 보기 ▸ 분할 보기, ⌥⌘S / Ctrl+Alt+S, the status-bar `columns-2` button): the page area splits
  좌우 (default) or 위아래 into two panes of the **same** document, 1 px `--border-default` between them. Each pane has
  its own zoom, zoom mode, view rotation, current page and scroll; layout (단일 / 연속 / 두 쪽) and night mode are
  shared. The second pane opens on the focused pane's page at its zoom. The **focused** pane (2 px `--accent-subtle`
  inset outline) is the one the status bar shows and the keys, menus, sidebar navigation and search hits act on;
  it alone carries the annotation / form / 편집 layers — the other pane is a read-only view until a click in it
  focuses it (a text selection or a pan starts with that click; a drawing tool, whose layer moves over with the
  focus, starts with the next one). 동기화 스크롤 (status bar `link-2`, off by default): a
  pane that scrolls moves the other by the same number of pages (page units, so any zoom), keeping the distance the
  user put between them. Esc with nothing else to cancel (no tool armed, no text selection, no annotation
  selected or being edited) closes the split, as do the menu item, the button and opening another document; the
  main pane stays where it was. Both panes share one tile manager (one in-flight budget) and the engine's cache.

---

## 6. Tool modes, cursors, click behaviour

| Mode | Tool | Cursor | Drag / click | Esc → |
|---|---|---|---|---|
| 읽기 | 선택(텍스트) | `text` | drag selects text; click clears | — |
| 읽기 | 손 | `grab`/`grabbing` | pans | 선택 |
| 읽기 | 스냅샷 | `crosshair` | marquee copies the region as an image | 선택 |
| 주석 | 형광펜 / 밑줄 / 취소선 / 물결선 | `text` + colour dot | drag over text → markup annotation on the selected line runs | 선택 |
| 주석 | 메모 | `copy` | click → sticky note, popover opens focused | 선택 |
| 주석 | 펜 | `crosshair` (1 px dot) | drag → ink stroke; ⇧ constrains to a straight line | 선택 |
| 주석 | 지우개 | circle sized to the eraser | drag removes whole ink strokes it touches | 펜 |
| 주석 | 사각형 / 타원 | `crosshair` | drag; ⇧ = square/circle; ⌥ = from centre | 선택 |
| 주석 | 선 / 화살표 | `crosshair` | drag; ⇧ snaps to 15° | 선택 |
| 주석 | 텍스트 상자 | `crosshair` | drag a box or click for auto-size, then edit inline | 선택 |
| 주석 | 도장 / 서명 | ghost preview follows the cursor (a built-in 도장 shows its label) | click places, drag sizes; first use opens 도장 선택 (결재 · 승인 · 기밀, APPROVED · FINAL · DRAFT · CONFIDENTIAL, 이미지 선택…) / 서명 만들기 (그리기 · 입력 · 이미지 + 저장된 서명); the panel's 도장 변경… / 서명 변경… reopens it | 선택 |
| 편집 | 선택 | arrow (`move` over an object) | hover outlines the object under the pointer (the smallest one); click selects, ⇧click adds/removes; a drag on empty space draws a marquee that selects the objects wholly inside it (⇧ adds; a plain click deselects); drag moves (on drop: one `transform_object` per selected object, in order, as one gesture — a part-way failure toasts "개체 N개 중 M개만 이동했습니다"); corner handles scale (⇧ or an image keeps the aspect); double-click on text opens 문단 편집; ⌫/⌦ deletes, arrows nudge 1 pt (⇧ 10); ⌘D duplicates 10 pt right and down, ⌘C / ⌘V copy and paste onto the page in view (each further paste onto a page one more 10 pt step; another page through `duplicate_objects` `targetPage`; an object that cannot go there toasts, as does a clipboard whose objects changed) — the copies are selected; a 읽기 전용 / 이동만 가능 object shows its badge + reason and refuses the gesture | 선택 해제 |
| 편집 | 텍스트 수정 | `text` over text | click → 문단 편집 (Stage 7): the whole paragraph (`probe_paragraph`) opens in an editing box over its own area — the original masked with the page paper, the text at the paragraph's size × zoom, leading, alignment, colour and an approximate font family; a right-edge handle sets the width; a floating bar has 크기 · 색상 · 정렬 (왼쪽/가운데/오른쪽/양쪽 맞춤) · 취소 · 완료. Mixed styles show 서식이 하나로 통일됩니다. On a rotated page or view the box (mask, text, width handle) turns with the page, so the text reads the way it will be written; the bar stays upright. 완료 / ⌘↵ / a click outside commits with reflow (`edit_paragraph`, one undo step 문단 편집), Esc cancels. Characters the paragraph's font lacks ask once to switch to the SeePDF 한글 글꼴. **The content below follows the paragraph** (Stage 9): while typing, once the text runs more than half a line past the original box a dashed line marks the original bottom and a notice says 완료하면 아래 내용이 밀립니다 (the paper-backed box covers what is below — it is not deleted); 텍스트 추가 never shows either. 완료 first asks `edit_paragraph` for a `push` dry run (nothing written), then writes with `push` — the in-column content that follows moves down by the growth (up when the paragraph got shorter), with a toast 아래 내용을 N pt 옮겼습니다 / 당겼습니다 + 실행 취소 (the edit and the moves are one undo step). When something in the way (a figure or table across the column, a footer, a box around the paragraph) or the page's bottom margin stops the push **and** the paragraph spacing cannot absorb the rest — i.e. something would really overlap — the prompt 문단이 들어갈 자리가 부족합니다 (§10) asks first; with nothing below the paragraph it says the text would run past the page's bottom margin instead. Pending 영역 표시 marks over the content that moved move with it (a mark half over moved content is dropped with 문서가 바뀌어 표시한 영역을 지웠습니다). Every call of one 완료 is pinned to the generation the paragraph was found at: when the document changes meanwhile (undo / redo under the prompt or under the editor) nothing is written — the paragraph is looked up again (inside its first line where it was, then by that line's text wherever it moved), the **same** editor stays open with the typed text (편집하는 동안 문서가 바뀌어 저장하지 않았습니다…) and 완료 writes it; if it is gone, the editor still stays open with the typed text (이 문단을 더 이상 찾을 수 없습니다 … 복사해 두거나 Esc로 닫으세요). Emptying the box and pressing 완료 deletes the paragraph and pulls what followed it up into its place (one `edit_paragraph` with no text, toast 아래 내용을 N pt 당겼습니다 + 실행 취소). A page whose rewrite would drop an inline image or a gradient refuses paragraph editing up front (edit.reason.unwritableContent); a write the engine refuses after the probe accepted the paragraph never drops what was typed — the toast says why and the editor stays open; when only the content below cannot be moved on that page, the prompt offers 겹쳐서 그대로 쓰기 instead of pushing. A commit that arrives mid-composition (a click outside) waits for the IME to finish the syllable. A refused paragraph toasts its reason | 선택 |
| 편집 | 텍스트 추가 | `crosshair` | click → the same editing box, empty, in the tool default (12 pt black unless `toolDefaults.addText`); 완료 writes `add_text_object` sized to the typed text | 선택 |
| 편집 | 이미지 추가 | `crosshair` | drag a box, or click for a 240 pt box at the click (kept on the page) → PNG/JPEG picker → placed inside the box with its aspect kept | 선택 |
| 편집 | 영역 표시 | `crosshair` | drag marks a region, snapped to the whole of every text run it crosses (a run is removed whole, so the box covers it) and keeping its own extent elsewhere — shown that way while dragging; a click on text marks that run's bounds (`list_page_objects`); every mark is clipped to the page box; a click on a mark selects it (× / ⌫ removes). Marks are pending (nothing is written), kept per page, drawn hatched in every 편집 tool, and survive tool switches; leaving 편집, closing the document or opening another with marks asks 표시한 영역을 버릴까요?; undo / redo / page ops / OCR drop them (toast). Text the preview says goes although it reaches outside a mark is outlined amber | 선택 |
| 페이지 | — | arrow | grid: click selects, drag reorders, double-click opens that page in 읽기 | — |
| 양식 | 채우기 | arrow / `text` over text fields / `pointer` over buttons | click focuses the field (HTML overlay input) | — |

Cursors are CSS `url(...) 12 12, <native fallback>` with 1× and 2× SVG data URIs. **Sticky tools:**
tapping a tool key latches it, holding it switches momentarily and reverts on release (Figma behaviour).

---

## 7. Right properties panel (260 px, auto-opens, never in 읽기)

| Context | Contents |
|---|---|
| Markup tool armed / markup selected | 8 colour swatches + custom, 불투명도 slider, 메모 textarea, 작성자, 만든 날짜, 삭제, 이 스타일을 기본값으로 |
| Any drawing tool, nothing selected (P1-12) | the controls edit **that tool's** default (remembered per tool in `Settings.toolDefaults`); 기본값으로 재설정 appears once it differs from the built-in style. With a selection, 이 스타일을 기본값으로 sets the default of the tool that drew it (사각형 → 사각형) |
| 펜 / 지우개 | 색상, 굵기 (1/2/4/8/12), 불투명도, 지우개 크기 |
| 도형 | 선 색상, 채우기 색상 (+ 채우기 없음), 굵기, 시작/끝 화살표 (line only), 불투명도 |
| 텍스트 상자 / 도장 | 글꼴 (bundled Hangul / Helvetica), 크기, 색상, 정렬, 채우기, 불투명도 |
| 텍스트 객체 (편집) | 글꼴 (read-only) + editability badge and reason, 크기 chips and 색상 swatches (`edit_text_object`, editable objects only), 위치 · 크기 fields X · Y · 너비 · 높이 (PDF points from the bottom left; Enter / blur commits, Esc restores; X / Y move, 너비 / 높이 scale about the bottom-left — `transform_object`), 삭제; several objects: count, X · Y of their bounding box (moves them all as one gesture), 삭제 |
| 이미지 객체 | 위치 X/Y, 크기 W/H (비율 고정), 삭제 — in 편집 the same X · Y · 너비 · 높이 fields as a text object |
| 페이지 선택 (페이지 mode) | 페이지 크기, 회전, 회전/삭제/추출/복제 buttons |
| 양식 필드 focused | 필드 이름 (read-only), 유형, 값, 필수 여부, 값 지우기 |
| 영역 표시 pending | 채우기 색상 (검정 · 회색 · 흰색, 기본 검정), 덮어쓸 문구, 표시된 영역 {{count}}개, 제거될 내용 per marked page (`redact_preview`, 250 ms debounce: text / image / annotation counts, the text runs, **collateral** in amber with why — PDFium cannot split a text object), **적용** (destructive, confirms "removed from the document; undo only until you save"; disabled with the reason when a mark covers a form field) → ONE `apply_redactions_batch` with every marked page (one undo step 영역 삭제); any failure leaves every mark — `verifyFailed` toasts that the document was restored; 표시 모두 지우기 |

A second surface exists for speed: an **inline popover** 8 px above a selected annotation (280 px wide,
`--radius-lg`, `--elevation-2`) with only swatches, opacity, thickness, 메모 and 삭제. It closes on Esc,
outside click, or a scroll of more than 40 px; opening the panel closes the popover.

**Threads (P2).** The 메모 popover opens 8 px **below** its note and ends with the note's thread: every reply
(작성자 in the semibold weight or 작성자 없음, relative date, the text; 답글 편집 `pencil` edits it in place —
uncontrolled, written on blur or ⌘↵ / Ctrl+↵, Esc cancels — and 답글 삭제 `x` deletes it), then a 2-line reply box
(답글 입력…) with 답글 달기 (also ⌘↵ / Ctrl+↵). A reply is written as 설정 ▸ 작성자. 답글 on any other annotation
(context menu) opens the **thread popover** below it: 답글 스레드 + ×, the annotation's own 메모 as the first line,
the same list and box, the box focused. Deleting an annotation that has replies (삭제 anywhere: popover, 주석 row,
⌫, context menu, the eraser) asks 주석과 답글 삭제 — "이 주석에 달린 답글 N개도 함께 삭제됩니다" (danger 삭제 /
취소); the replies go with it in the same undo step. An encrypted document cannot take replies (toast
`annot.thread.encrypted`).

---

## 8. Status bar (28 px)

`◀ [page input] / [total] ▶` · view layout segmented (단일 / 연속 / 두 쪽) · 왼쪽/오른쪽 회전 ·
야간 모드 `moon` (cycles, pressed while on) · 분할 보기 `columns-2` (pressed while split; while split also 위아래로
나누기 `rows-2` ⇄ 좌우로 나누기 `columns-2` and 동기화 스크롤 `link-2`, pressed while on — P2; the page field, layout,
rotation and zoom act on the focused pane) · zoom `− [slider] +` with a numeric combo (25/50/75/100/125/150/200/400 %, 페이지 맞춤, 너비 맞춤,
실제 크기) · right side: save state (저장됨 / 저장되지 않은 변경 사항 / 저장 중…) and a progress slot used
by OCR, export, search and save (label + determinate bar + cancel ×).

---

## 9. Page organizer (⌘4)

* The canvas is replaced by a responsive grid; the sidebar auto-collapses. Cell width follows a slider
  in the status bar (80 / 120 / 160 / 220 px). Layout comes from page sizes before any pixel exists, so
  cells never reflow.
* Cell = page image + page-number chip + rotation badge + selection ring. Hover reveals rotate-left,
  rotate-right and delete on the cell, and a `+` between cells for "여기에 삽입".
* Selection: click, ⇧click range, ⌘click toggle, marquee on empty space, ⌘A.
* Drag: a 2 px vertical insertion caret between cells; a multi-selection drags as a stacked ghost with a
  count badge. Dropping a PDF or an image from the OS inserts at the caret.
* Right-side action list: 회전 · 삭제 · 추출… · 복제 · 빈 페이지 삽입 · 파일에서 삽입… · 순서 뒤집기 · 분할…
* Everything is undoable and lives in the in-memory document until save. The status bar shows
  `변경됨 · 페이지 148 → 143`.

---

## 10. Dialogs

**OCR** (modal sheet, 520 px): 범위 (모든 페이지 / 현재 페이지 / 페이지 범위 `1,3,5-9`) · 언어 chips
(한국어 + English selected by default) · 출력 (검색 가능한 PDF) · 옵션 (인식 엔진 자동/Apple Vision/Tesseract —
only where `ocr_capabilities` lists `vision`; 자동 = Apple Vision there, 이미 텍스트가 있는 페이지 건너뛰기
**on**, 해상도 자동/200/300/400 DPI, 고급 ▸ 레이아웃 자동/단일 단/단일 블록 for Tesseract) · footer 예상 시간 + 취소/시작.
While running it becomes a progress view: `12 / 148`, a thumbnail of the page being processed,
elapsed/remaining, and an always-live 취소. On completion: an inline success bar with 실행 취소.

**여러 파일 OCR** (P1-7, 640 px, 도구 ▸ 여러 파일 OCR… or ⋯; no shortcut): 파일 추가… (multi-select) / 목록 비우기,
a list 파일 · 상태 (대기 / 여는 중… / 진행 중 n/m 페이지 / 저장 중… / 완료 · `<name>-ocr.pdf` / 건너뜀 · 이유 /
실패 · 이유 / 취소됨) with × per row, the OCR sheet's shared options (언어 chips, 인식 엔진, 해상도, 이미 텍스트가 있는 페이지
건너뛰기), 저장 위치 원본과 같은 폴더 (default) | 다른 폴더 + 찾아보기…. 시작 processes every file without a copy yet,
one at a time, the pages of each in parallel on one worker pool; each file is opened beside the window's document,
recognised, saved as `<name>-ocr.pdf` (never over the source or an existing file: ` (2)`, ` (3)`…) and closed. An
encrypted file asks 암호 입력; dismissing it skips the file. A file whose pages all have text is skipped with no
copy. While running: `파일 2/5 · name` + progress + 취소 (stops the current file, closes it, leaves the rest 대기;
시작 resumes). 닫기 only hides the sheet — the batch continues in the status bar (× cancels) and ends with a toast.
When done: 완료 N · 건너뜀 N · 실패 N and Finder에서 보기 (the first copy).

**여러 파일에서 검색** (P2, 640 px, 편집 ▸ 여러 파일에서 검색…, ⋯, the 검색 panel's button; works with no document
open; no shortcut): the query field (Enter starts) with 대소문자 구분 / 단어 단위로 · 검색할 파일: 파일 추가… (multi-select)
/ 폴더 추가… (every PDF below it, sub-folders included, `list_pdf_files`) / 목록 비우기 and 파일 N개 · the list, one row per
file: disclosure chevron, name, status (대기 / 여는 중… / 검색 중… / 결과 N개 / 건너뜀 · 이유 / 실패 · 이유 / 취소됨), ×; under
an open row its hits streaming in as they are found — `N쪽` + ±40 characters with the match marked (50 per file, then
더 보기). 검색 runs **one** job over every file in list order (the status bar shows `파일 2/5` with ×): each file is
opened beside the window's document, searched from its first page and closed; the window's own document, when
listed, is searched in memory (its unsaved edits count) and stays open; an encrypted file asks 암호 입력 and
dismissing it skips the file. While running: `파일 2/5 · name` + progress + 취소 (stops the file being read, closes it,
leaves the rest 대기). When done: 결과 N개 · 파일 N개 · 건너뜀 N · 실패 N. A click on a hit opens its file through the
normal open path (저장하지 않은 변경 사항 first; the password the search was given is tried first), closes the dialog and
makes the hits the 검색 panel's — the same page highlights, that hit current and scrolled into view, ⌘G walks them.
닫기 only hides the dialog: the job carries on in the status bar and ends with a toast (결과 보기 reopens the dialog,
whose results stay until 목록 비우기).

**내보내기** (non-modal sheet): format list on the left (PDF 평면화 / PNG / JPEG / 텍스트), options on the
right (페이지 범위, DPI 72–600 default 150, 품질, 투명 배경, 페이지마다 파일 하나), an estimated size, then
progress in the status bar and a completion toast with Finder에서 보기 / 폴더 열기.

**워터마크 / 머리글·바닥글** (P1-4, 760 px, ⋯ menu or ⌥⌘W / Ctrl+Alt+W): 종류 segmented (워터마크 | 머리글 |
바닥글) — the role picks the defaults (워터마크: 가운데, 45°, 60 pt, 25 %; 머리글: 위 가운데 `{{filename}}`;
바닥글: 아래 가운데 `{{page}} / {{total}}`; both 0°, 10 pt, 100 %), switching keeps text the user typed ·
내용 텍스트 | 이미지 (PNG/JPEG picker + 너비 pt) · text area with token chips 쪽 번호 / 전체 쪽수 / 날짜 /
파일 이름 inserted at the caret · 글자 크기 · 색상 (the Inspector swatches) · 불투명도 slider · 회전 (워터마크 only)
· 위치 3×3 anchor grid + 여백 pt · 페이지 범위 · a page-shaped preview on the right (CSS approximation over the
first page of the range: its thumbnail at its visual size — crop box, `/Rotate` 90/270 swaps the sides — tokens
expanded for that page; an image stamp shows the picked file at its real aspect when the webview can load it through
the asset protocol, else a dashed placeholder). 적용 → one undo step (`undo.watermark` / `undo.headerFooter`) and a
toast with 실행 취소. **기존 항목 제거** (Stage 8): 워터마크 제거 / 머리글 제거 / 바닥글 제거 (the current role) and
모두 제거 (every SeePDF stamp, including ones from before roles were recorded) over the same 페이지 범위 →
`remove_stamps`, one undo step `undo.removeStamps`, a toast `N개 항목을 제거했습니다` with 실행 취소 or 제거할 항목이
없습니다 (info); the dialog stays open.

**압축** (P1-5, 640 px, 도구 ▸ 압축… or ⋯): 이미지 품질 radio 인쇄 품질 · 300 DPI / 화면용 · 150 DPI (default) /
최소 크기 · 96 DPI · 페이지 범위 · 예상 runs `compress_estimate` on a scratch copy with an inline progress bar and
취소, then a table 현재 크기 / 압축 후 / 변화 (signed %) / 줄인 이미지. When the result is not smaller the change is
amber and an amber note advises against applying. 적용 (disabled until an estimate exists) replaces the document
as one `undo.compress` step and toasts `before → after` with 실행 취소. Changing an option, 다시 예상 or closing
the dialog discards the pending result.

**문서 비교** (P1-6, 480 px, 도구 ▸ 문서 비교… or ⋯): 현재 문서 (A) · 비교할 파일 (B) + 찾아보기… · 대소문자 무시 ·
비교 opens B with `open_document` **beside** the window's document (never replacing it; an encrypted B goes
through 암호 입력 and the dialog comes back with its state), runs `compare_documents` with an inline progress bar
(`비교 중… 3/12`) and 취소 (which also closes B), then switches to the **compare view**: full window over the shell
(under dialogs and toasts), a header with 변경된 페이지 N · 삽입 N단어 · 삭제 N단어, 이전 변경 / 다음 변경 (no
wrap-around; disabled at the ends), 변경만 보기 and 닫기; column captions A / B with the file names; one scroller
in which every row holds one page pair (A left, B right, each fit to half the width), so both sides scroll together
by construction. Pages are paired by text similarity (Stage 8 `alignPages: true`), so a page only B has is its own
row badged 삽입된 페이지 (green; the empty A side says the same) and a page only A has one badged 삭제된 페이지 (red).
Row caption `A 3쪽 · B 3쪽` with `+N −N` or 변경 없음. A document closed mid-job ends the run quietly (`cancelled`,
no error toast). Deleted / replaced words are tinted red on A, inserted / replaced words green on B (one rect per
line fragment, PDF points mapped through the crop box and `/Rotate` like the viewer). 닫기 or Esc closes the
view and B, and so does closing the window or replacing the window's document; the document shortcuts are off
while it is open.

**복구** (P1-8, 640 px, at launch in the main window only when `list_recovery` is non-empty): intro line, a table
이름 / 저장 시각 (locale date + time) / 페이지 / 크기 with 열기 and 삭제 per row, footer 모두 삭제 (danger, left) and
나중에 (keeps the copies for the next launch). 열기 opens the copy through the normal open path (password retry
included) under the original name (`open_document { displayName }`, so the title is not `<uuid>.pdf`) and toasts
복구 사본입니다. 다른 이름으로 저장하세요; that document's 저장 goes to 다른 이름으로 저장 (suggesting the original
name), its recovery path never enters 최근 항목, and saving it elsewhere discards the entry. 저장 안 함 on it asks
복구 사본을 보관할까요? — 보관 (default, Esc) keeps the entry for the next launch, 삭제 discards it.
**자동 저장**: every `autosaveSec` (설정 ▸ 일반 ▸ 자동 저장 간격 끄기 / 30초 / 1분 (default) / 5분) a dirty
document whose generation moved since the last copy is written with `write_recovery`; 저장, 다른 이름으로 저장
and a clean close (저장 / 저장 안 함 / nothing to save) call `clear_recovery`, as does an undo that brings the
document back to clean. A failed copy toasts once per document. While an annotation drag has something hidden, a
beat waits for the drop, ⌘S is queued until it and ⌘Z / ⇧⌘Z are ignored (nothing may capture the transient HIDDEN
bit); image / foreign stamps and image signatures are hidden too, the ghost painting an engine snapshot of them.

**문단이 들어갈 자리가 부족합니다** (Stage 9, 380 px, `askChoice`): one line saying what blocks the push and how many
pt would overlap (the paragraph spacing already counted: a push the spacing absorbs never asks), then the answers
stacked full width — 글자 크기 줄여 맞추기 (N%) (accent; N from a `fit` dry run, disabled with a note when even 70 %
does not fit; writes with `fit`, toasts 글자 크기를 N%로 줄여 맞췄습니다) · 아래로 밀고 남은 부분은 겹치기 (writes with
`push` — moved as far as there is room — and toasts how much still overlaps) — and 계속 편집 in the footer. When the
content below cannot be moved on this page at all (its content stream cannot be rewritten), the line says 이 페이지에서는
아래 내용을 옮길 수 없어 N pt가 겹칩니다 and the second answer is 겹쳐서 그대로 쓰기 (writes with `overlap`). With
nothing below the paragraph (`pastBottomPt`), the line says 아래에는 다른 내용이 없지만, 문단이 페이지 아래 여백을 N pt
넘어갑니다, the second answer is 그대로 쓰기 (페이지 아래로 넘침) and its toast 문단이 페이지 아래 여백을 N pt 넘었습니다 (also Esc and a click on the backdrop: back to the box with the typed text,
nothing written). The first enabled answer has the focus. While any prompt is open, 편집 mode's own keys (Esc, ⌫,
arrows, ⌘C / ⌘V / ⌘D) stay off, so Esc answers the prompt instead of cancelling the editor underneath. Leaving
편집 (another mode, 페이지 정리, closing or replacing the document) commits an open editor first, so the prompt can
appear there too; 계속 편집 (or a declined font) keeps the mode and the typed text.

**Others**: 암호 입력 (on `passwordRequired`, retries in place) · 저장하지 않은 변경 사항 (저장 / 저장 안 함 /
취소) · 파일 합치기 (ordered list with drag, per-file range field, warnings for forms/outline) ·
문서 분할 ({{n}}쪽마다 / 페이지 범위로, output folder) · 인쇄 (range + the system dialog) ·
설정 (일반 / 모양 / 주석 / 고급) · 문서 정보 · 여러 파일을 어떻게 열까요? (각각 열기 / 하나로 합치기).

All dialogs are rendered in the webview, `--elevation-3`, `--radius-xl`, Esc closes, the primary button
is the accent one and is 36 px tall.

---

## 11. Welcome / recent screen

Two columns. Left (220 px): wordmark, version, `파일 열기…`, `최근 항목`, a quiet 도움말 / 설정.
Right: recent files as a 3–5 column grid of cards — first-page thumbnail (from `/recent-thumb`),
filename, folder, relative date (오늘 / 어제 / 3일 전), page count, size; right-click → 열기 /
Finder에서 보기 / 목록에서 제거 / 즐겨찾기; a filter field; pinned items first.
The whole window is a drop target; multiple dropped PDFs ask 각각 열기 / 하나로 합치기.
Empty state: a large dashed drop zone with `PDF 파일을 여기에 놓으세요` and `또는 ⌘O로 열기`.

---

## 12. Context menus (all rendered in the webview, themed and localised)

* **Text selection**: 복사 · 형광펜 · 밑줄 · 취소선 · 메모 추가 · 영역 표시로 표시 · 검색 — at the top of the canvas
  menu while a text selection exists. 복사 writes the text; 형광펜 / 밑줄 / 취소선 make the same markup the tool
  would (one per page, the tool's remembered style) and clear the selection; 메모 추가 puts a note just after
  the selection's last line and opens it; 영역 표시로 표시 marks the line rects and switches to 편집 · 영역 표시;
  검색 puts the selection (whitespace folded, ≤ 200 characters) into the 검색 panel and runs it
* **Empty page area**: 붙여넣기 · 메모 추가 · 페이지 회전 · 이미지로 내보내기 · 스냅샷 · 페이지로 이동…
* **Annotation**: 편집 · 속성… · 메모 열기 · 복사 · 삭제 · 이 스타일을 기본값으로. P2: a right-click on an annotation
  on the canvas (hit-tested in the pane it happened in; replies are never on the page) opens the canvas menu with
  **답글** (the thread popover / the note's popover, reply box focused; 읽기 switches to 주석) and 삭제 on top; a 주석
  tab row's menu is 답글 · 삭제
* **Thumbnail / page cell**: 이 페이지로 이동 · 왼쪽/오른쪽 회전 · 삭제 · 복제 · 추출… · 뒤에 페이지 삽입… ·
  이미지로 내보내기
* **Outline item**: 이동 · 하위 항목 모두 펼치기/접기
* **Search result**: 이동 · 복사 · 이 결과 형광펜
* **Form field**: 값 지우기 · 모든 필드 지우기 · 필드 강조 표시 전환

---

## 13. Keyboard shortcuts

`⌘` → Ctrl on Windows, `⌥` → Alt, unless a row says otherwise.

**File** — 열기 ⌘O / Ctrl+O · 최근 항목 열기 ⇧⌘O / Ctrl+Shift+O · 닫기 ⌘W / Ctrl+W · 저장 ⌘S / Ctrl+S ·
다른 이름으로 저장 ⇧⌘S / Ctrl+Shift+S · 내보내기 ⌥⌘E / Ctrl+Alt+E · 인쇄 ⌘P / Ctrl+P ·
문서 정보 ⌘I / Ctrl+D · 설정 ⌘, / Ctrl+, · 종료 ⌘Q / Alt+F4

**Edit** — 실행 취소 ⌘Z / Ctrl+Z · 다시 실행 ⇧⌘Z / Ctrl+Y · 잘라내기·복사·붙여넣기 ⌘X/⌘C/⌘V ·
전체 선택 ⌘A / Ctrl+A · 삭제 ⌫ / Delete · 복제 ⌘D / Ctrl+D (페이지 mode; 편집 · 선택 duplicates objects) · 찾기 ⌘F / Ctrl+F ·
다음/이전 찾기 ⌘G / ⇧⌘G, F3 / Shift+F3

**View** — 사이드바 ⌃⌘S / Ctrl+F9 · 속성 패널 ⌥⌘P / Ctrl+F10 · 사이드바 탭 ⌘⌥1…4 / Ctrl+Alt+1…4 ·
확대/축소 ⌘+ / ⌘− · 실제 크기 ⌘0 · 페이지 맞춤 ⌘9 · 너비 맞춤 ⌘8 · 단일/연속/두 쪽 ⌃1/⌃2/⌃3 (Ctrl+Shift+1/2/3) ·
왼쪽/오른쪽 회전 ⌘L / ⌘R · 야간 모드 ⌃⌘N / Ctrl+Shift+N · 읽기 모드 ⌃⌘R / F8 · 전체 화면 ⌃⌘F / F11 ·
분할 보기 ⌥⌘S / Ctrl+Alt+S (P2; toggles — Esc with nothing else to cancel closes it)

> 읽기 모드 (P1-12) hides the title/tool bar, sidebar, 주석 tool strip, properties panel and status bar;
> only the pages remain, and a toast says "읽기 모드 · Esc 키를 누르면 나갑니다". 전체 화면 is the window's own
> (`setFullscreen`; on macOS the View menu's native item). They combine, and Esc in 읽기 모드 leaves both.

**Navigation** — 다음/이전 페이지 ↓ ↑ PageDown PageUp Space ⇧Space · 첫/마지막 페이지 ⌘↑ / ⌘↓
(Ctrl+Home / Ctrl+End) · 페이지로 이동 ⌥⌘G / Ctrl+G · 뒤로/앞으로 ⌘[ / ⌘] (Alt+← / Alt+→)

**Modes and tools** (only while the canvas has focus and no input is editing) — 모드 ⌘1…⌘5 ·
선택 V · 손 Space(hold) · 형광펜 H · 밑줄 U · 취소선 K · 메모 N · 펜 P · 지우개 E · 사각형 R · 타원 O ·
선 L · 화살표 A · 텍스트 상자 T · 도장 S · 서명 G · 영역 표시 ⇧R · 도구 해제 Esc

**Pages mode** — 회전 ⌘L / ⌘R · 삭제 ⌫ · 추출 ⌥⌘X / Ctrl+Alt+X · 페이지 삽입 ⌥⌘I / Ctrl+Alt+I · 모두 선택 ⌘A

> `Space` pages down on a tap (< 200 ms, no movement) and pans on hold+drag. `H` is the highlighter, not
> the hand tool.

---

## 14. Design tokens

### 14.1 Typography

```css
--font-ui: "Pretendard Variable", Pretendard, -apple-system, BlinkMacSystemFont, "Apple SD Gothic Neo",
           "Segoe UI Variable Text", "Segoe UI", "Malgun Gothic", system-ui, sans-serif;
--font-mono: ui-monospace, SFMono-Regular, "SF Mono", "Cascadia Mono", Menlo, "D2Coding", monospace;
```

Pretendard Variable (OFL) ships as a Korean subset woff2 (~350 kB, `font-display: swap`, loaded off the
critical path) because `-apple-system` resolves to Apple SD Gothic Neo on macOS and Malgun Gothic on
Windows — two Korean faces with different metrics. Weights 400/500/600 only. Korean body text uses
`word-break: keep-all; line-height: 1.45`.

| Token | Size / line-height / weight | Use |
|---|---|---|
| `--text-xs` | 11/15/500 | badges, page chips, timestamps |
| `--text-sm` | 12/17/400 | sidebar lists, status bar |
| `--text-base` | 13/19/400 | default UI text, menus, panel labels |
| `--text-md` | 14/21/500 | dialog body, tool labels |
| `--text-lg` | 17/24/600 | dialog titles |
| `--text-xl` | 22/30/600 | welcome title |

`[data-os="windows"] :root { font-size: 14px }`; everything else is rem-relative.

### 14.2 Spacing, radius, elevation

4 px grid: `--space-0.5` 2 · `--space-1` 4 · `--space-2` 8 · `--space-3` 12 · `--space-4` 16 ·
`--space-5` 20 · `--space-6` 24 · `--space-8` 32 · `--space-12` 48.
Controls: 28 compact / 32 default / 36 dialog primary; icon buttons 28×28 with 20 px icons; never below
a 24 px hit target.
Radius: `--radius-sm` 4 (chips, swatches) · `--radius-md` 6 (buttons, inputs) · `--radius-lg` 10
(popovers, cards) · `--radius-xl` 14 (dialogs, sheets).
Elevation: `--elevation-1` `0 1px 2px rgb(0 0 0/.06), 0 0 0 1px var(--border-subtle)` ·
`--elevation-2` `0 6px 16px rgb(0 0 0/.12), 0 0 0 1px var(--border-subtle)` ·
`--elevation-3` `0 16px 48px rgb(0 0 0/.20), 0 0 0 1px var(--border-subtle)` ·
`--page-shadow` `0 1px 3px rgb(0 0 0/.18), 0 0 0 1px rgb(0 0 0/.06)`.

### 14.3 Colour

```css
:root {                                   /* light */
  --accent:#2F6FEB; --accent-hover:#2A62D0; --accent-subtle:#E8F0FE; --accent-contrast:#FFFFFF;
  --bg-app:#F5F5F7; --bg-chrome:#FAFAFB; --bg-panel:#FFFFFF; --bg-canvas:#9DA2AA;
  --bg-hover:rgb(0 0 0/.045); --bg-active:rgb(0 0 0/.075); --bg-selected:#E8F0FE;
  --text-primary:#1A1C1F; --text-secondary:#5C6169; --text-tertiary:#8A9099;
  --text-inverse:#FFFFFF; --text-disabled:#B3B8BF;
  --border-subtle:rgb(0 0 0/.08); --border-default:rgb(0 0 0/.14); --border-strong:rgb(0 0 0/.24);
  --focus-ring:0 0 0 2px #FFFFFF, 0 0 0 4px rgb(47 111 235/.55);
  --danger:#D93F3F; --danger-subtle:#FDECEC; --warning:#C77700; --success:#2E9E5B;
}
:root[data-theme="dark"], @media (prefers-color-scheme: dark) :root:not([data-theme="light"]) {
  --accent:#4E8BFF; --accent-hover:#6B9EFF; --accent-subtle:#1B2942; --accent-contrast:#0B0D10;
  --bg-app:#1B1D20; --bg-chrome:#232629; --bg-panel:#1F2225; --bg-canvas:#121417;
  --bg-hover:rgb(255 255 255/.06); --bg-active:rgb(255 255 255/.10); --bg-selected:#1B2942;
  --text-primary:#E8EAED; --text-secondary:#A2A8B0; --text-tertiary:#737A83;
  --text-inverse:#14161A; --text-disabled:#565C64;
  --border-subtle:rgb(255 255 255/.09); --border-default:rgb(255 255 255/.16); --border-strong:rgb(255 255 255/.28);
  --focus-ring:0 0 0 2px #1F2225, 0 0 0 4px rgb(78 139 255/.6);
  --danger:#F1706E; --danger-subtle:#37211F; --warning:#E0A030; --success:#4FBE7C;
}
```

`--text-tertiary` (3.2:1 light / 3.6:1 dark) is only for non-essential metadata at ≥ 12 px, never labels.

### 14.4 Annotation palette (the 8 swatches)

| # | ko / en | Hex | Highlight alpha |
|---|---|---|---|
| 1 | 노랑 / Yellow | `#FFD84D` | .40 |
| 2 | 연두 / Green | `#7BD64A` | .38 |
| 3 | 하늘 / Blue | `#4DB8FF` | .38 |
| 4 | 분홍 / Pink | `#FF7FB0` | .38 |
| 5 | 보라 / Purple | `#B07BFF` | .35 |
| 6 | 주황 / Orange | `#FF9A3D` | .38 |
| 7 | 빨강 / Red | `#F5533D` | .32 |
| 8 | 검정 / Black | `#2B2F33` | ink/shape only |

Swatches are stored opaque; Highlight applies the alpha column, every other type defaults to opacity 1.0
with its own slider. The alpha reaches the PDF as `/CA` through the annotation's colour (every
`FPDFAnnot_SetColor` rewrites `/CA`, so stroke and fill are always written with the same alpha).

### 14.5 Motion

`--dur-instant` 90 ms (hover/press) · `--dur-fast` 140 ms (popovers, panels) · `--dur-slow` 220 ms
(mode change, sheets) · `--ease-out` `cubic-bezier(.22,.61,.36,1)` · `--ease-inout`
`cubic-bezier(.4,0,.2,1)`. Never animate scroll or the page bitmap. `prefers-reduced-motion: reduce`
collapses everything to 0 ms except 90 ms opacity fades.

### 14.6 Icons

`lucide-react`, per-icon imports, stroke width 1.75, 20 px in the toolbar and 16 px in lists/menus,
`currentColor`. Map: `mouse-pointer-2` 선택 · `hand` · `crop` 스냅샷 · `highlighter` · `underline` ·
`strikethrough` · `squiggle`→`waves` 물결선 · `message-square-plus` 메모 · `pen-line` 펜 · `eraser` ·
`square` · `circle` · `minus` 선 · `move-up-right` 화살표 · `type` 텍스트 상자 · `stamp` · `signature` ·
`image-plus` · `square-pen` 텍스트 수정 · `square-dashed` 영역 표시 · `panel-left` · `panel-right` ·
`rectangle-vertical` 축소판 · `list-tree` 목차 · `message-square` 주석 · `search` · `rotate-ccw` ·
`rotate-cw` · `zoom-in` · `zoom-out` · `moon` 야간 · `scan-text` OCR · `lock` 보안 · `download` 내보내기 ·
`printer` · `settings` · `undo-2` / `redo-2` · `layout-grid` 페이지.
Where lucide is wrong for a PDF concept (도장, 서명) draw two custom 20×20 SVGs on lucide's grid
(24 box, 2 px padding, 1.75 stroke, round caps) so they sit in the same family.

### 14.7 The "not templated" rules

1. One accent colour, used only for state, never for decoration.
2. No shadow on anything that is not floating; panels use a 1 px border.
3. Borders are alpha-on-background, never a fixed grey.
4. Radii step with elevation: 6 flat, 10 floating, 14 sheets. Never mix 8 and 12.
5. Icon-only buttons always get a tooltip after 500 ms with the shortcut appended.
6. The selected thumbnail uses a 2 px accent ring and a bolder page number, never a filled block.

---

## 15. i18n catalogue (ko = default and design reference, en = second)

Flat dotted keys in `src/i18n/ko.json` and `src/i18n/en.json`. `{{placeholders}}` must be identical in
both locales; `_other` keys exist only because English pluralises (Korean repeats the singular).
`scripts/check-i18n.mjs` fails CI on any key or placeholder drift.
**405 keys**: the 373 of `ux-research.md` §5 plus the 32 additions in §15.20 required by this spec.

### 15.1 `app.*`
| Key | ko | en |
|---|---|---|
| `app.name` | SeePDF | SeePDF |
| `app.untitled` | 제목 없음 | Untitled |
| `app.window.document` | {{name}} — SeePDF | {{name}} — SeePDF |
| `app.edited` | 편집됨 | Edited |
| `app.version` | 버전 {{version}} | Version {{version}} |

### 15.2 `menu.*`
| Key | ko | en |
|---|---|---|
| `menu.file` | 파일 | File |
| `menu.file.open` | 열기… | Open… |
| `menu.file.openRecent` | 최근 항목 열기 | Open Recent |
| `menu.file.clearRecent` | 메뉴 지우기 | Clear Menu |
| `menu.file.close` | 닫기 | Close |
| `menu.file.save` | 저장 | Save |
| `menu.file.saveAs` | 다른 이름으로 저장… | Save As… |
| `menu.file.export` | 내보내기… | Export… |
| `menu.file.print` | 인쇄… | Print… |
| `menu.file.docInfo` | 문서 정보 | Document Properties |
| `menu.file.revealInFinder` | Finder에서 보기 | Reveal in Finder |
| `menu.file.revealInExplorer` | 파일 탐색기에서 보기 | Show in Explorer |
| `menu.edit` | 편집 | Edit |
| `menu.edit.undo` | 실행 취소 | Undo |
| `menu.edit.redo` | 다시 실행 | Redo |
| `menu.edit.cut` | 잘라내기 | Cut |
| `menu.edit.copy` | 복사 | Copy |
| `menu.edit.paste` | 붙여넣기 | Paste |
| `menu.edit.delete` | 삭제 | Delete |
| `menu.edit.duplicate` | 복제 | Duplicate |
| `menu.edit.selectAll` | 전체 선택 | Select All |
| `menu.edit.deselect` | 선택 해제 | Deselect |
| `menu.edit.find` | 찾기… | Find… |
| `menu.edit.findNext` | 다음 찾기 | Find Next |
| `menu.edit.findPrevious` | 이전 찾기 | Find Previous |
| `menu.view` | 보기 | View |
| `menu.view.sidebar` | 사이드바 | Sidebar |
| `menu.view.inspector` | 속성 패널 | Inspector |
| `menu.view.zoomIn` | 확대 | Zoom In |
| `menu.view.zoomOut` | 축소 | Zoom Out |
| `menu.view.actualSize` | 실제 크기 | Actual Size |
| `menu.view.fitPage` | 페이지에 맞춤 | Fit Page |
| `menu.view.fitWidth` | 너비에 맞춤 | Fit Width |
| `menu.view.readingMode` | 읽기 모드 | Reading Mode |
| `menu.view.night` | 야간 모드 | Night Mode |
| `menu.view.fullScreen` | 전체 화면 | Full Screen |
| `menu.go` | 이동 | Go |
| `menu.go.nextPage` | 다음 페이지 | Next Page |
| `menu.go.previousPage` | 이전 페이지 | Previous Page |
| `menu.go.firstPage` | 첫 페이지 | First Page |
| `menu.go.lastPage` | 마지막 페이지 | Last Page |
| `menu.go.goToPage` | 페이지로 이동… | Go to Page… |
| `menu.go.back` | 뒤로 | Back |
| `menu.go.forward` | 앞으로 | Forward |
| `menu.tools` | 도구 | Tools |
| `menu.tools.ocr` | 텍스트 인식(OCR)… | Recognize Text (OCR)… |
| `menu.tools.security` | 보안… | Security… |
| `menu.tools.redact` | 영역 표시 | Redact |
| `menu.tools.compress` | 압축… | Compress… |
| `menu.tools.compare` | 문서 비교… | Compare Documents… |
| `menu.tools.merge` | 파일 합치기… | Merge Files… |
| `menu.tools.batchOcr` | 여러 파일 OCR… | Batch OCR… |
| `menu.window` | 윈도우 | Window |
| `menu.window.minimize` | 최소화 | Minimize |
| `menu.window.zoom` | 확대/축소 | Zoom |
| `menu.help` | 도움말 | Help |
| `menu.help.shortcuts` | 단축키 | Keyboard Shortcuts |
| `menu.help.about` | SeePDF 정보 | About SeePDF |
| `menu.settings` | 설정… | Settings… |
| `menu.quit` | SeePDF 종료 | Quit SeePDF |

### 15.3 `mode.*`
| Key | ko | en |
|---|---|---|
| `mode.read` | 읽기 | Read |
| `mode.annotate` | 주석 | Annotate |
| `mode.edit` | 편집 | Edit |
| `mode.pages` | 페이지 | Pages |
| `mode.form` | 양식 | Fill |

### 15.4 `tool.*`
| Key | ko | en |
|---|---|---|
| `tool.select` | 선택 | Select |
| `tool.hand` | 손 도구 | Hand |
| `tool.snapshot` | 스냅샷 | Snapshot |
| `tool.highlight` | 형광펜 | Highlight |
| `tool.underline` | 밑줄 | Underline |
| `tool.strikeout` | 취소선 | Strikethrough |
| `tool.note` | 메모 | Note |
| `tool.pen` | 펜 | Pen |
| `tool.eraser` | 지우개 | Eraser |
| `tool.rectangle` | 사각형 | Rectangle |
| `tool.ellipse` | 타원 | Ellipse |
| `tool.line` | 선 | Line |
| `tool.arrow` | 화살표 | Arrow |
| `tool.textbox` | 텍스트 상자 | Text Box |
| `tool.stamp` | 도장 | Stamp |
| `tool.signature` | 서명 | Signature |
| `tool.addText` | 텍스트 추가 | Add Text |
| `tool.addImage` | 이미지 추가 | Add Image |
| `tool.editText` | 텍스트 수정 | Edit Text |
| `tool.redact` | 영역 표시 | Redact |
| `tool.hint.momentary` | 키를 누르고 있으면 임시로 전환됩니다 | Hold the key for a temporary switch |

### 15.5 `sidebar.*`
| Key | ko | en |
|---|---|---|
| `sidebar.toggle` | 사이드바 표시/숨기기 | Show/Hide Sidebar |
| `sidebar.tab.thumbnails` | 축소판 | Thumbnails |
| `sidebar.tab.outline` | 목차 | Outline |
| `sidebar.tab.annotations` | 주석 | Annotations |
| `sidebar.tab.search` | 검색 | Search |
| `sidebar.outline.empty` | 이 문서에는 목차가 없습니다 | This document has no outline |
| `sidebar.annotations.empty` | 주석이 없습니다 | No annotations yet |
| `sidebar.annotations.filter` | 유형별 필터 | Filter by type |
| `sidebar.annotations.count` | 주석 {{count}}개 | {{count}} annotation |
| `sidebar.annotations.count_other` | 주석 {{count}}개 | {{count}} annotations |
| `sidebar.search.placeholder` | 문서에서 검색 | Search in document |
| `sidebar.search.matchCase` | 대소문자 구분 | Match case |
| `sidebar.search.wholeWord` | 단어 단위로 | Whole word |
| `sidebar.search.empty` | 검색 결과가 없습니다 | No results |
| `sidebar.search.results` | 결과 {{count}}개 | {{count}} result |
| `sidebar.search.results_other` | 결과 {{count}}개 | {{count}} results |
| `sidebar.search.searching` | 검색 중… | Searching… |
| `sidebar.search.pageLabel` | {{page}}쪽 | Page {{page}} |

### 15.6 `view.*`
| Key | ko | en |
|---|---|---|
| `view.layout.single` | 단일 | Single |
| `view.layout.continuous` | 연속 | Continuous |
| `view.layout.twoPage` | 두 쪽 | Two Pages |
| `view.zoom.in` | 확대 | Zoom in |
| `view.zoom.out` | 축소 | Zoom out |
| `view.zoom.fitPage` | 페이지 맞춤 | Fit page |
| `view.zoom.fitWidth` | 너비 맞춤 | Fit width |
| `view.zoom.actual` | 실제 크기 | Actual size |
| `view.rotateLeft` | 왼쪽으로 회전 | Rotate left |
| `view.rotateRight` | 오른쪽으로 회전 | Rotate right |
| `view.night.label` | 야간 모드 | Night mode |
| `view.night.off` | 끄기 | Off |
| `view.night.dark` | 어둡게 | Dark |
| `view.night.sepia` | 세피아 | Sepia |
| `view.pageOf` | {{current}} / {{total}} | {{current}} of {{total}} |
| `view.goToPage.title` | 페이지로 이동 | Go to Page |
| `view.goToPage.placeholder` | 페이지 번호 | Page number |

### 15.7 `prop.*`
| Key | ko | en |
|---|---|---|
| `prop.title` | 속성 | Properties |
| `prop.empty` | 선택된 항목이 없습니다 | Nothing selected |
| `prop.color` | 색상 | Color |
| `prop.fillColor` | 채우기 색상 | Fill color |
| `prop.strokeColor` | 선 색상 | Stroke color |
| `prop.noFill` | 채우기 없음 | No fill |
| `prop.opacity` | 불투명도 | Opacity |
| `prop.thickness` | 굵기 | Thickness |
| `prop.lineStyle` | 선 스타일 | Line style |
| `prop.lineStyle.solid` | 실선 | Solid |
| `prop.lineStyle.dashed` | 점선 | Dashed |
| `prop.arrowStart` | 시작 화살표 | Start arrow |
| `prop.arrowEnd` | 끝 화살표 | End arrow |
| `prop.font` | 글꼴 | Font |
| `prop.fontSize` | 크기 | Size |
| `prop.bold` | 굵게 | Bold |
| `prop.italic` | 기울임 | Italic |
| `prop.align` | 정렬 | Alignment |
| `prop.align.left` | 왼쪽 | Left |
| `prop.align.center` | 가운데 | Center |
| `prop.align.right` | 오른쪽 | Right |
| `prop.note` | 메모 | Note |
| `prop.note.placeholder` | 메모를 입력하세요 | Type a note |
| `prop.author` | 작성자 | Author |
| `prop.created` | 만든 날짜 | Created |
| `prop.modified` | 수정한 날짜 | Modified |
| `prop.setDefault` | 이 스타일을 기본값으로 | Set as default style |
| `prop.bringToFront` | 맨 앞으로 | Bring to Front |
| `prop.sendToBack` | 맨 뒤로 | Send to Back |
| `prop.position` | 위치 | Position |
| `prop.size` | 크기 | Size |
| `prop.lockAspect` | 비율 고정 | Lock aspect ratio |
| `prop.eraserSize` | 지우개 크기 | Eraser size |

### 15.8 `pages.*`
| Key | ko | en |
|---|---|---|
| `pages.title` | 페이지 정리 | Organize Pages |
| `pages.selected` | {{count}}쪽 선택됨 | {{count}} page selected |
| `pages.selected_other` | {{count}}쪽 선택됨 | {{count}} pages selected |
| `pages.rotateLeft` | 왼쪽으로 회전 | Rotate Left |
| `pages.rotateRight` | 오른쪽으로 회전 | Rotate Right |
| `pages.delete` | 삭제 | Delete |
| `pages.duplicate` | 복제 | Duplicate |
| `pages.extract` | 추출… | Extract… |
| `pages.insertBlank` | 빈 페이지 삽입 | Insert Blank Page |
| `pages.insertFromFile` | 파일에서 삽입… | Insert from File… |
| `pages.insertHere` | 여기에 삽입 | Insert here |
| `pages.reverse` | 순서 뒤집기 | Reverse Order |
| `pages.split` | 분할… | Split… |
| `pages.merge` | 파일 합치기… | Merge Files… |
| `pages.moveTo` | 위치 이동… | Move to… |
| `pages.thumbnailSize` | 축소판 크기 | Thumbnail size |
| `pages.deleteConfirm` | 선택한 {{count}}쪽을 삭제할까요? | Delete {{count}} selected page(s)? |
| `pages.deleteWarning` | 저장하기 전까지는 실행 취소할 수 있습니다. | You can undo this until you save. |
| `pages.extract.title` | 페이지 추출 | Extract Pages |
| `pages.extract.asNewFile` | 새 파일로 저장 | Save as a new file |
| `pages.extract.removeAfter` | 추출 후 원본에서 삭제 | Delete from original after extracting |
| `pages.split.title` | 문서 분할 | Split Document |
| `pages.split.everyN` | {{n}}쪽마다 분할 | Split every {{n}} pages |
| `pages.split.byRange` | 페이지 범위로 분할 | Split by page range |
| `pages.changed` | 변경됨 · 페이지 {{from}} → {{to}} | Changed · {{from}} → {{to}} pages |

### 15.9 `form.*`
| Key | ko | en |
|---|---|---|
| `form.highlightFields` | 필드 강조 표시 | Highlight fields |
| `form.clearField` | 값 지우기 | Clear field |
| `form.clearAll` | 모든 필드 지우기 | Clear all fields |
| `form.flatten` | 양식 평면화 | Flatten form |
| `form.flattenWarning` | 평면화하면 입력란을 더 이상 수정할 수 없습니다. | Flattening makes fields permanently uneditable. |
| `form.required` | 필수 항목 | Required |
| `form.fieldName` | 필드 이름 | Field name |
| `form.fieldType` | 유형 | Type |
| `form.noFields` | 이 문서에는 입력 가능한 양식이 없습니다 | This document has no fillable fields |
| `form.fieldCount` | 입력란 {{count}}개 | {{count}} field |
| `form.fieldCount_other` | 입력란 {{count}}개 | {{count}} fields |

### 15.10 `sign.*`
| Key | ko | en |
|---|---|---|
| `sign.title` | 서명 | Signature |
| `sign.create` | 서명 만들기 | Create Signature |
| `sign.draw` | 그리기 | Draw |
| `sign.type` | 입력 | Type |
| `sign.image` | 이미지 | Image |
| `sign.drawHint` | 아래 영역에 서명하세요 | Sign in the area below |
| `sign.typePlaceholder` | 이름을 입력하세요 | Type your name |
| `sign.chooseImage` | 이미지 선택… | Choose Image… |
| `sign.clear` | 지우기 | Clear |
| `sign.saved` | 저장된 서명 | Saved signatures |
| `sign.save` | 이 서명 저장 | Save this signature |
| `sign.delete` | 서명 삭제 | Delete signature |
| `sign.placeHint` | 페이지를 클릭하여 서명을 배치하세요 | Click the page to place your signature |
| `sign.style.script` / `.hand` / `.formal` | 필기체 / 손글씨 / 정자체 | Script / Handwriting / Formal |
| `sign.style.krBrush` / `.krMyungjo` / `.krPen` (a Hangul name; only styles whose fonts are installed) | 궁서체 / 명조 / 펜글씨 | Gungsuh / Myungjo / Pen |
| `sign.libraryFull` | 보관함이 가득 찼습니다 (최대 {{max}}개) | The library is full (up to {{max}}) |
| `stampPick.title` | 도장 선택 | Choose a Stamp |
| `prop.resetDefault` | 기본값으로 재설정 | Reset to default |

### 15.11 `ocr.*`
| Key | ko | en |
|---|---|---|
| `ocr.title` | 텍스트 인식 (OCR) | Recognize Text (OCR) |
| `ocr.description` | 스캔된 페이지에서 글자를 인식하여 검색할 수 있게 만듭니다. | Recognize text on scanned pages so the document becomes searchable. |
| `ocr.range` | 범위 | Range |
| `ocr.range.all` | 모든 페이지 | All pages |
| `ocr.range.current` | 현재 페이지 | Current page |
| `ocr.range.custom` | 페이지 범위 | Page range |
| `ocr.range.placeholder` | 예: 1, 3, 5-9 | e.g. 1, 3, 5-9 |
| `ocr.range.invalid` | 페이지 범위를 확인해 주세요 | Check the page range |
| `ocr.language` | 언어 | Languages |
| `ocr.language.ko` | 한국어 | Korean |
| `ocr.language.en` | English | English |
| `ocr.language.ja` | 日本語 | Japanese |
| `ocr.language.zh` | 中文(简体) | Chinese (Simplified) |
| `ocr.output` | 출력 | Output |
| `ocr.output.searchable` | 검색 가능한 PDF | Searchable PDF |
| `ocr.output.searchableHint` | 원본 이미지는 그대로 두고 보이지 않는 텍스트 레이어를 추가합니다. | Keeps the original image and adds an invisible text layer. |
| `ocr.options` | 옵션 | Options |
| `ocr.option.autoRotate` | 페이지 회전 자동 감지 | Detect page rotation |
| `ocr.option.skipText` | 이미 텍스트가 있는 페이지 건너뛰기 | Skip pages that already have text |
| `ocr.option.dpi` | 해상도 | Resolution |
| `ocr.dpi.auto` | 자동 | Auto |
| `ocr.estimate` | 예상 시간: 약 {{seconds}}초 | Estimated time: about {{seconds}}s |
| `ocr.start` | 시작 | Start |
| `ocr.progress` | {{current}} / {{total}}쪽 처리 중 | Processing page {{current}} of {{total}} |
| `ocr.remaining` | 남은 시간 약 {{seconds}}초 | About {{seconds}}s remaining |
| `ocr.done` | 텍스트 레이어가 추가되었습니다. 이제 검색할 수 있습니다. | Text layer added. The document is now searchable. |
| `ocr.doneCount` | {{count}}쪽을 인식했습니다 | Recognized {{count}} pages |
| `ocr.cancelled` | 텍스트 인식이 취소되었습니다 | Text recognition cancelled |
| `ocr.failed` | 텍스트 인식에 실패했습니다 | Text recognition failed |
| `ocr.noImagePages` | 인식할 스캔 페이지가 없습니다 | No scanned pages to recognize |

#### `batchOcr.*` (P1-7, Stage 6a)
| Key | ko | en |
|---|---|---|
| `batchOcr.title` | 여러 파일 텍스트 인식 (OCR) | Batch Text Recognition (OCR) |
| `batchOcr.description` | 여러 PDF의 스캔 페이지를 차례로 인식해 검색 가능한 사본을 만듭니다. 원본 파일은 바꾸지 않습니다. | Recognizes the scanned pages of several PDFs, one file after another, and saves searchable copies. The original files are never changed. |
| `batchOcr.pick` | 텍스트를 인식할 PDF 선택 | Choose PDFs to recognize |
| `batchOcr.add` / `batchOcr.clear` | 파일 추가… / 목록 비우기 | Add Files… / Clear List |
| `batchOcr.count` | 파일 {{count}}개 | {{count}} files |
| `batchOcr.empty` | 추가한 파일이 없습니다 | No files added yet |
| `batchOcr.remove` | {{name}} 제거 | Remove {{name}} |
| `batchOcr.col.file` / `batchOcr.col.status` | 파일 / 상태 | File / Status |
| `batchOcr.status.queued` · `.opening` · `.saving` | 대기 · 여는 중… · 저장 중… | Waiting · Opening… · Saving… |
| `batchOcr.status.running` | 진행 중 {{done}}/{{total}} 페이지 | In progress: page {{done}} of {{total}} |
| `batchOcr.status.done` · `.skipped` · `.failed` · `.cancelled` | 완료 · 건너뜀 · 실패 · 취소됨 | Done · Skipped · Failed · Cancelled |
| `batchOcr.reason.password` | 암호를 입력하지 않았습니다 | No password was entered |
| `batchOcr.output` | 저장 위치 | Save To |
| `batchOcr.output.beside` / `.folder` | 원본과 같은 폴더 / 다른 폴더 | Same folder as the original / Another folder |
| `batchOcr.output.pickFolder` | 사본을 저장할 폴더 선택 | Choose a folder for the copies |
| `batchOcr.output.noFolder` | 선택한 폴더 없음 | No folder chosen |
| `batchOcr.output.hint` | 파일 이름 뒤에 -ocr을 붙여 저장합니다. 같은 이름의 파일이 있으면 (2), (3)…을 붙입니다. | Saved as <name>-ocr.pdf. If that name is taken, (2), (3)… is added. |
| `batchOcr.progress` | 파일 {{done}}/{{total}} | File {{done}} of {{total}} |
| `batchOcr.running` | 여러 파일 텍스트 인식 중… | Recognizing text in several files… |
| `batchOcr.summary` | 완료 {{done}} · 건너뜀 {{skipped}} · 실패 {{failed}} | {{done}} done · {{skipped}} skipped · {{failed}} failed |
| `batchOcr.finished` | 여러 파일 텍스트 인식을 마쳤습니다: 완료 {{done}} · 건너뜀 {{skipped}} · 실패 {{failed}} | Batch text recognition finished: {{done}} done · {{skipped}} skipped · {{failed}} failed |
| `batchOcr.cancelled` | 여러 파일 텍스트 인식을 취소했습니다: 완료 {{done}} · 건너뜀 {{skipped}} · 실패 {{failed}} | Batch text recognition cancelled: {{done}} done · {{skipped}} skipped · {{failed}} failed |

### 15.12 `export.*`
| Key | ko | en |
|---|---|---|
| `export.title` | 내보내기 | Export |
| `export.format` | 형식 | Format |
| `export.format.pdfFlattened` | PDF (평면화) | PDF (flattened) |
| `export.format.pdfCompressed` | PDF (압축) | PDF (compressed) |
| `export.format.png` | PNG 이미지 | PNG image |
| `export.format.jpeg` | JPEG 이미지 | JPEG image |
| `export.format.text` | 텍스트 (.txt) | Text (.txt) |
| `export.flattenAnnotations` | 주석 평면화 | Flatten annotations |
| `export.flattenForms` | 양식 평면화 | Flatten form fields |
| `export.quality` | 품질 | Quality |
| `export.quality.high` | 최고 품질 | Highest quality |
| `export.quality.balanced` | 균형 | Balanced |
| `export.quality.small` | 최소 크기 | Smallest size |
| `export.estimatedSize` | 예상 크기: {{size}} | Estimated size: {{size}} |
| `export.dpi` | 해상도 (DPI) | Resolution (DPI) |
| `export.transparentBg` | 투명 배경 | Transparent background |
| `export.onePerPage` | 페이지마다 파일 하나 | One file per page |
| `export.singleImage` | 하나의 이미지로 이어 붙이기 | Stitch into a single image |
| `export.preserveLayout` | 레이아웃 유지 시도 | Try to preserve layout |
| `export.button` | 내보내기 | Export |
| `export.progress` | 내보내는 중… {{current}} / {{total}} | Exporting… {{current}} of {{total}} |
| `export.done` | {{name}}(으)로 내보냈습니다 | Exported to {{name}} |
| `export.revealFinder` | Finder에서 보기 | Reveal in Finder |
| `export.revealExplorer` | 폴더 열기 | Open folder |
| `export.failed` | 내보내기에 실패했습니다 | Export failed |

### 15.13 `security.*` and `redact.*`
| Key | ko | en |
|---|---|---|
| `security.title` | 보안 | Security |
| `security.password.open` | 문서 열기 암호 | Open password |
| `security.password.owner` | 권한 암호 | Permissions password |
| `security.password.set` | 암호 설정 | Set password |
| `security.password.remove` | 암호 제거 | Remove password |
| `security.password.confirm` | 암호 확인 | Confirm password |
| `security.password.mismatch` | 암호가 일치하지 않습니다 | Passwords do not match |
| `security.password.required` | 이 문서는 암호로 보호되어 있습니다 | This document is password protected |
| `security.password.enter` | 암호를 입력하세요 | Enter the password |
| `security.password.wrong` | 암호가 올바르지 않습니다 | Incorrect password |
| `security.permissions` | 권한 | Permissions |
| `security.permission.print` | 인쇄 허용 | Allow printing |
| `security.permission.copy` | 내용 복사 허용 | Allow copying content |
| `security.permission.modify` | 문서 수정 허용 | Allow modifying the document |
| `security.permission.annotate` | 주석 추가 허용 | Allow annotating |
| `security.encryption` | 암호화 | Encryption |
| `security.removeMetadata` | 메타데이터 제거 | Remove metadata |
| `security.removeMetadataHint` | 작성자, 제목, 생성 프로그램 등의 정보를 지웁니다. | Clears author, title, producer and similar fields. |
| `redact.title` | 영역 표시 | Redaction |
| `redact.markArea` | 영역 표시하기 | Mark area |
| `redact.apply` | 표시한 영역 적용 | Apply redactions |
| `redact.applyWarning` | 표시한 내용이 문서에서 제거됩니다. 저장하기 전까지만 실행 취소할 수 있습니다. | The marked content is removed from the document. You can undo this only until you save. |
| `redact.overlayText` | 덮어쓸 문구 | Overlay text |
| `redact.marked` | 표시된 영역 {{count}}개 | {{count}} marked area |
| `redact.marked_other` | 표시된 영역 {{count}}개 | {{count}} marked areas |
| `redact.done` | {{count}}개 영역이 제거되었습니다 | {{count}} area removed |
| `redact.done_other` | {{count}}개 영역이 제거되었습니다 | {{count}} areas removed |
| `redact.empty` | 영역을 드래그하거나 텍스트를 클릭해 표시하세요 | Drag an area or click text to mark it |
| `redact.overlayPlaceholder` | 예: 비공개 | e.g. REDACTED |
| `redact.previewTitle` | 제거될 내용 | What will be removed |
| `redact.pageLabel` | {{page}}쪽 | Page {{page}} |
| `redact.pageSummary` | {{page}}쪽 · 텍스트 {{text}} · 이미지 {{images}} · 주석 {{annots}} | Page {{page}} · {{text}} text · {{images}} images · {{annots}} annotations |
| `redact.previewLoading` | 확인하는 중… | Checking… |
| `redact.previewFailed` | 미리 보기를 불러오지 못했습니다 | Could not load the preview |
| `redact.collateral` | 표시한 영역 밖의 텍스트도 함께 제거됩니다 | Text outside the marks will also be removed |
| `redact.collateralHint` | PDF 텍스트 개체는 나눌 수 없어, 표시한 영역에 걸친 텍스트 개체가 통째로 제거됩니다. | A PDF text object cannot be split, so any text object a mark touches is removed whole. |
| `redact.formFields` | 양식 필드가 있는 영역은 적용할 수 없습니다: {{names}} | Areas with form fields cannot be redacted: {{names}} |
| `redact.clearAll` | 표시 모두 지우기 | Clear all marks |
| `redact.removeMark` | 표시 지우기 | Remove mark |
| `redact.confirmTitle` | 표시한 영역을 적용할까요? | Apply redactions? |
| `redact.verifyFailed` | 내용이 완전히 제거되었는지 확인하지 못해 문서를 원래대로 되돌렸습니다 | The removal could not be verified, so the document was restored |
| `redact.leaveTitle` | 표시한 영역을 버릴까요? | Discard the marked areas? |
| `redact.leaveBody` | 아직 적용하지 않은 영역 표시가 사라집니다. | Marks you have not applied yet will be lost. |
| `redact.discard` | 버리기 | Discard |
| `redact.marksDropped` | 문서가 바뀌어 표시한 영역을 지웠습니다 | The document changed, so the marks were cleared |
| `redact.markSelection` | 영역 표시로 표시 | Mark for redaction |

### 15.14 `welcome.*`
| Key | ko | en |
|---|---|---|
| `welcome.open` | 파일 열기… | Open File… |
| `welcome.recent` | 최근 항목 | Recent |
| `welcome.recent.empty` | 최근에 연 문서가 없습니다 | No recent documents |
| `welcome.recent.search` | 최근 항목 검색 | Search recent |
| `welcome.recent.remove` | 목록에서 제거 | Remove from list |
| `welcome.recent.pin` | 즐겨찾기에 추가 | Pin |
| `welcome.recent.unpin` | 즐겨찾기에서 제거 | Unpin |
| `welcome.dropZone` | PDF 파일을 여기에 놓으세요 | Drop a PDF here |
| `welcome.dropZone.hint` | 또는 {{shortcut}}로 열기 | or press {{shortcut}} to open |
| `welcome.multipleFiles` | 여러 파일을 어떻게 열까요? | How should these files open? |
| `welcome.openSeparately` | 각각 열기 | Open separately |
| `welcome.mergeIntoOne` | 하나로 합치기 | Merge into one |
| `welcome.pageCount` | {{count}}쪽 | {{count}} page |
| `welcome.pageCount_other` | {{count}}쪽 | {{count}} pages |

### 15.15 `settings.*`
| Key | ko | en |
|---|---|---|
| `settings.title` | 설정 | Settings |
| `settings.tab.general` | 일반 | General |
| `settings.tab.appearance` | 모양 | Appearance |
| `settings.tab.annotation` | 주석 | Annotations |
| `settings.tab.advanced` | 고급 | Advanced |
| `settings.language` | 언어 | Language |
| `settings.language.ko` | 한국어 | 한국어 |
| `settings.language.en` | English | English |
| `settings.language.restart` | 언어를 바꾸면 일부 메뉴는 다시 시작한 후에 적용됩니다. | Some menus update after a restart. |
| `settings.theme` | 테마 | Theme |
| `settings.theme.system` | 시스템 설정 | System |
| `settings.theme.light` | 밝게 | Light |
| `settings.theme.dark` | 어둡게 | Dark |
| `settings.defaultView` | 기본 보기 | Default view |
| `settings.restorePosition` | 마지막으로 본 위치 기억 | Remember last reading position |
| `settings.autosave` | 자동 저장 간격 | Autosave interval |
| `settings.authorName` | 주석 작성자 이름 | Annotation author name |
| `settings.renderQuality` | 렌더링 품질 | Rendering quality |
| `settings.cacheSize` | 캐시 크기 | Cache size |
| `settings.clearCache` | 캐시 비우기 | Clear cache |
| `settings.resetDefaults` | 기본값으로 되돌리기 | Reset to defaults |

### 15.16 `common.*` and `dialog.*`
| Key | ko | en |
|---|---|---|
| `common.ok` | 확인 | OK |
| `common.cancel` | 취소 | Cancel |
| `common.save` | 저장 | Save |
| `common.dontSave` | 저장 안 함 | Don't Save |
| `common.delete` | 삭제 | Delete |
| `common.apply` | 적용 | Apply |
| `common.close` | 닫기 | Close |
| `common.done` | 완료 | Done |
| `common.continue` | 계속 | Continue |
| `common.retry` | 다시 시도 | Try Again |
| `common.undo` | 실행 취소 | Undo |
| `common.more` | 더 보기 | More |
| `common.loading` | 불러오는 중… | Loading… |
| `common.processing` | 처리 중… | Working… |
| `common.today` | 오늘 | Today |
| `common.yesterday` | 어제 | Yesterday |
| `common.daysAgo` | {{count}}일 전 | {{count}} days ago |
| `dialog.unsaved.title` | "{{name}}"의 변경 사항을 저장하시겠습니까? | Save changes to "{{name}}"? |
| `dialog.unsaved.body` | 저장하지 않으면 변경 사항이 사라집니다. | Your changes will be lost if you don't save them. |
| `dialog.docInfo.title` | 문서 정보 | Document Properties |
| `dialog.docInfo.fileName` | 파일 이름 | File name |
| `dialog.docInfo.location` | 위치 | Location |
| `dialog.docInfo.fileSize` | 파일 크기 | File size |
| `dialog.docInfo.pageSize` | 페이지 크기 | Page size |
| `dialog.docInfo.pdfVersion` | PDF 버전 | PDF version |
| `dialog.docInfo.producer` | 생성 프로그램 | Producer |
| `dialog.docInfo.tagged` | 태그된 PDF | Tagged PDF |
| `dialog.docInfo.searchable` | 검색 가능 | Searchable |

### 15.17 `status.*` and `error.*`
| Key | ko | en |
|---|---|---|
| `status.saved` | 저장됨 | Saved |
| `status.unsaved` | 저장되지 않은 변경 사항 | Unsaved changes |
| `status.saving` | 저장 중… | Saving… |
| `status.rendering` | 페이지를 그리는 중… | Rendering… |
| `status.readOnly` | 읽기 전용 | Read-only |
| `status.encrypted` | 암호화됨 | Encrypted |
| `error.openFailed` | 파일을 열 수 없습니다 | Could not open the file |
| `error.corrupted` | 손상된 PDF 파일입니다 | The PDF file is damaged |
| `error.notPdf` | PDF 파일이 아닙니다 | This is not a PDF file |
| `error.fileMissing` | 파일을 찾을 수 없습니다. 이동되었거나 삭제되었을 수 있습니다. | The file could not be found. It may have been moved or deleted. |
| `error.saveFailed` | 저장하지 못했습니다 | Could not save |
| `error.readOnlyFile` | 이 파일은 읽기 전용입니다. 다른 이름으로 저장해 주세요. | This file is read-only. Save it under a different name. |
| `error.permissionDenied` | 이 문서는 해당 작업을 허용하지 않습니다 | This document does not allow that operation |
| `error.outOfMemory` | 메모리가 부족합니다. 다른 창을 닫고 다시 시도해 주세요. | Out of memory. Close other windows and try again. |
| `error.engineCrashed` | PDF 엔진에 문제가 발생했습니다. 문서를 다시 불러옵니다. | The PDF engine failed. Reloading the document. |
| `error.generic` | 문제가 발생했습니다 | Something went wrong |
| `error.details` | 자세히 | Details |

### 15.18 `a11y.*`
| Key | ko | en |
|---|---|---|
| `a11y.page` | {{n}}쪽 | Page {{n}} |
| `a11y.pageThumbnail` | {{n}}쪽 축소판 | Thumbnail of page {{n}} |
| `a11y.currentPage` | 현재 페이지 | Current page |
| `a11y.annotation` | {{type}} 주석, {{author}} | {{type}} annotation by {{author}} |
| `a11y.closePanel` | 패널 닫기 | Close panel |
| `a11y.progress` | 진행률 {{percent}}퍼센트 | {{percent}} percent complete |
| `a11y.colorSwatch` | 색상 {{name}} | Color {{name}} |
| `a11y.canvas` | 문서 보기 영역 | Document view |

### 15.19 Colour names (`color.*`, used by `a11y.colorSwatch` and the swatch tooltips)
| Key | ko | en |
|---|---|---|
| `color.yellow` | 노랑 | Yellow |
| `color.green` | 연두 | Green |
| `color.blue` | 하늘 | Blue |
| `color.pink` | 분홍 | Pink |
| `color.purple` | 보라 | Purple |
| `color.orange` | 주황 | Orange |
| `color.red` | 빨강 | Red |
| `color.black` | 검정 | Black |
| `color.custom` | 사용자 지정 | Custom |

### 15.20 Additions required by this specification (new keys, 32)
| Key | ko | en |
|---|---|---|
| `tool.squiggly` | 물결선 | Squiggly |
| `annot.kind.line` | 선 | Line |
| `annot.kind.arrow` | 화살표 | Arrow |
| `annot.kind.textbox` | 텍스트 상자 | Text box |
| `annot.externalAp` | 다른 앱에서 만든 주석입니다. 수정하면 모양이 다시 그려집니다. | Made in another app: editing regenerates its appearance. |
| `annot.lineAsInk` | 선과 화살표는 자유형 주석으로 저장됩니다. | Lines and arrows are saved as ink annotations. |
| `edit.font.substitute` | 글꼴을 {{font}}(으)로 대체합니다. 계속할까요? | Substitute {{font}} for this text. Continue? |
| `edit.readOnly.xobject` | 그룹(XObject) 안의 텍스트는 수정할 수 없습니다 | Text inside a group (XObject) cannot be edited |
| `edit.readOnly.noUnicode` | 이 글꼴에는 문자 정보가 없어 수정할 수 없습니다 | This font has no character map, so it cannot be edited |
| `edit.readOnly.type3` | Type3 글꼴은 수정할 수 없습니다 | Type3 fonts cannot be edited |
| `edit.overflow` | 새 텍스트가 원래 영역보다 넓습니다 | The new text is wider than the original run |
| `edit.badge.readOnly` | 읽기 전용 | Read-only |
| `pages.merge.formWarning` | 원본의 양식 필드는 유지되지 않을 수 있습니다 | Form fields of the source may not be preserved |
| `pages.merge.outlineWarning` | 원본의 목차는 유지되지 않습니다 | The outline of the source is not preserved |
| `pages.insertFrom.title` | 파일에서 페이지 삽입 | Insert Pages from File |
| `pages.split.outDir` | 저장 폴더 | Destination folder |
| `dialog.saveAs.readOnly` | 원본 파일을 저장할 수 없어 다른 이름으로 저장합니다. | The original file cannot be written, so Save As is used. |
| `dialog.merge.title` | 파일 합치기 | Merge Files |
| `dialog.merge.addFiles` | 파일 추가… | Add Files… |
| `dialog.recover.title` | 복구된 문서가 있습니다 | A recovered document is available |
| `dialog.recover.body` | 마지막으로 저장하지 않은 변경 사항을 복구할 수 있습니다. | Unsaved changes from the last session can be recovered. |
| `print.title` | 인쇄 | Print |
| `print.range` | 인쇄 범위 | Page range |
| `print.preparing` | 인쇄 준비 중… | Preparing to print… |
| `ocr.engine` | 인식 엔진 | Engine |
| `ocr.engine.auto` | 자동 | Automatic |
| `ocr.engine.vision` | Apple Vision | Apple Vision |
| `ocr.engine.tesseract` | Tesseract | Tesseract |
| `ocr.engine.autoHint` | 이 Mac에서는 Apple Vision으로 더 빠르고 정확하게 인식합니다 | On this Mac, Apple Vision recognizes text faster and more accurately |
| `ocr.layout` | 레이아웃 | Layout |
| `ocr.layout.auto` | 자동 | Automatic |
| `ocr.layout.column` | 단일 단 | Single column |
| `ocr.layout.block` | 단일 블록 | Single block |
| `status.ocr` | 텍스트 인식 중… | Recognizing text… |
| `error.busy` | 지금은 처리할 수 없습니다. 잠시 후 다시 시도해 주세요. | Busy right now. Please try again in a moment. |

### 15.20a Stage 9 — the content below an edited paragraph (`edit.flow.*`)
| Key | ko | en |
|---|---|---|
| `edit.flow.hint` | 완료하면 아래 내용이 밀립니다 | Content below moves down when you finish |
| `edit.flow.blockedTitle` | 문단이 들어갈 자리가 부족합니다 | Not Enough Room for the Paragraph |
| `edit.flow.blocked.pageBottom` | 아래 내용을 페이지 끝까지 밀어도 {{pt}}pt가 겹칩니다. | Even pushed to the bottom of the page, the content below would still overlap by {{pt}} pt. |
| `edit.flow.blocked.obstacle` | 아래에 있는 다른 내용(그림·표·바닥글 등) 때문에 더 밀 수 없어 {{pt}}pt가 겹칩니다. | Other content below (a figure, table or footer) stops the push, so {{pt}} pt would overlap. |
| `edit.flow.blocked.pastBottom` | 아래에는 다른 내용이 없지만, 문단이 페이지 아래 여백을 {{pt}}pt 넘어갑니다. | Nothing is below, but the paragraph would run {{pt}} pt past the page's bottom margin. |
| `edit.flow.keepPastBottom` | 그대로 쓰기 (페이지 아래로 넘침) | Keep It (Runs Past the Page Bottom) |
| `edit.flow.pastBottom` | 문단이 페이지 아래 여백을 {{pt}}pt 넘었습니다 | The paragraph runs {{pt}} pt past the page's bottom margin |
| `edit.flow.docChanged` | 편집하는 동안 문서가 바뀌어 저장하지 않았습니다. 문단을 다시 찾았으니 완료를 다시 눌러 주세요 | The document changed while you were editing, so nothing was saved. The paragraph was found again — press Done again |
| `edit.flow.paragraphGone` | 편집하는 동안 문서가 바뀌어 이 문단을 더 이상 찾을 수 없습니다. 입력한 내용은 편집 상자에 남아 있으니 복사해 두거나 Esc로 닫으세요 | The document changed while you were editing and this paragraph can no longer be found. What you typed is still in the editor — copy it, or press Esc to close |
| `edit.flow.blocked.unmovable` | 이 페이지에서는 아래 내용을 옮길 수 없어 {{pt}}pt가 겹칩니다. | The content below cannot be moved on this page, so {{pt}} pt would overlap. |
| `edit.flow.overlapInPlace` | 겹쳐서 그대로 쓰기 | Write It Overlapping |
| `edit.reason.unwritableContent` | 이 페이지의 인라인 이미지나 그라데이션은 다시 쓸 수 없어 문단을 편집하면 사라집니다. 이 문단은 편집할 수 없습니다 | This page's inline images or gradients cannot be rewritten and would be lost, so this paragraph cannot be edited |
| `edit.flow.fit` | 글자 크기 줄여 맞추기 ({{pct}}%) | Shrink text to fit ({{pct}}%) |
| `edit.flow.fitTooSmall` | 글자를 {{pct}}%까지 줄여도 원래 자리에 다 들어가지 않습니다. | Even at {{pct}}%, the text does not fit its original area. |
| `edit.flow.overlap` | 아래로 밀고 남은 부분은 겹치기 | Push down and overlap the rest |
| `edit.flow.keepEditing` | 계속 편집 | Keep Editing |
| `edit.flow.pushed` | 아래 내용을 {{pt}}pt 옮겼습니다 | Moved the content below down by {{pt}} pt |
| `edit.flow.pulled` | 아래 내용을 {{pt}}pt 당겼습니다 | Moved the content below up by {{pt}} pt |
| `edit.flow.pushedOverlap` | 아래 내용을 {{pt}}pt 옮겼지만 {{over}}pt는 겹칩니다 | Moved the content below down by {{pt}} pt; {{over}} pt still overlaps |
| `edit.flow.overlaps` | 문단이 아래 내용과 {{pt}}pt 겹칩니다 | The paragraph overlaps the content below by {{pt}} pt |
| `edit.flow.fitted` | 글자 크기를 {{pct}}%로 줄여 맞췄습니다 | Shrank the text to {{pct}}% to fit |

`edit.paragraph.overflow` (Stage 7's "may overlap the text below" toast) is gone: the flow above replaces it.

### 15.20b P2 — annotation threads, 여러 파일에서 검색, 분할 보기
| Key | ko | en |
|---|---|---|
| `menu.view.split` | 분할 보기 | Split View |
| `menu.edit.findInFiles` | 여러 파일에서 검색… | Search in Files… |
| `view.split.label` | 분할 보기 | Split view |
| `view.split.side` | 좌우로 나누기 | Split side by side |
| `view.split.stacked` | 위아래로 나누기 | Split top and bottom |
| `view.split.sync` | 동기화 스크롤 | Synchronized scrolling |
| `view.split.secondPane` | 분할 보기의 두 번째 창 | Second pane of the split view |
| `annot.thread.label` | 답글 스레드 | Thread |
| `annot.thread.reply` | 답글 | Reply |
| `annot.thread.placeholder` | 답글 입력… | Write a reply… |
| `annot.thread.send` | 답글 달기 | Reply |
| `annot.thread.edit` | 답글 편집 | Edit reply |
| `annot.thread.delete` | 답글 삭제 | Delete reply |
| `annot.thread.noAuthor` | 작성자 없음 | No author |
| `annot.thread.expand` | 답글 {{count}}개 펼치기 | Show {{count}} reply |
| `annot.thread.expand_other` | 답글 {{count}}개 펼치기 | Show {{count}} replies |
| `annot.thread.collapse` | 답글 {{count}}개 접기 | Hide {{count}} reply |
| `annot.thread.collapse_other` | 답글 {{count}}개 접기 | Hide {{count}} replies |
| `annot.thread.deleteTitle` | 주석과 답글 삭제 | Delete Annotation and Replies |
| `annot.thread.deleteBody` | 이 주석에 달린 답글 {{count}}개도 함께 삭제됩니다. 삭제할까요? | Its {{count}} reply will be deleted too. Delete? |
| `annot.thread.deleteBody_other` | 이 주석에 달린 답글 {{count}}개도 함께 삭제됩니다. 삭제할까요? | Its {{count}} replies will be deleted too. Delete? |
| `annot.thread.encrypted` | 암호가 걸린 문서에는 답글을 저장할 수 없습니다. 보안에서 암호를 제거한 뒤 다시 시도하세요. | Replies cannot be saved into an encrypted document. Remove the password in Security and try again. |
| `annot.thread.failed` | 답글을 저장하지 못했습니다 | The reply could not be saved |
| `undo.annotReply` | 답글 | Reply |
| `multiSearch.title` | 여러 파일에서 검색 | Search in Files |
| `multiSearch.query` | 검색어 | Search term |
| `multiSearch.files` | 검색할 파일 | Files |
| `multiSearch.addFiles` | 파일 추가… | Add Files… |
| `multiSearch.addFolder` | 폴더 추가… | Add Folder… |
| `multiSearch.pickFiles` | 검색할 PDF 선택 | Choose PDFs to search |
| `multiSearch.pickFolder` | 검색할 폴더 선택 (하위 폴더 포함) | Choose a folder to search (sub-folders included) |
| `multiSearch.empty` | 검색할 파일이나 폴더를 추가하세요 | Add files or a folder to search |
| `multiSearch.start` | 검색 | Search |
| `multiSearch.running` | 여러 파일에서 검색 중… | Searching in files… |
| `multiSearch.summary` | 결과 {{hits}}개 · 파일 {{files}}개 · 건너뜀 {{skipped}} · 실패 {{failed}} | {{hits}} results in {{files}} files · {{skipped}} skipped · {{failed}} failed |
| `multiSearch.finished` | 여러 파일에서 검색을 마쳤습니다: 결과 {{hits}}개 · 파일 {{files}}개 | Search in files finished: {{hits}} results in {{files}} files |
| `multiSearch.cancelled` | 여러 파일에서 검색을 취소했습니다: 결과 {{hits}}개 · 파일 {{files}}개 | Search in files cancelled: {{hits}} results in {{files}} files |
| `multiSearch.showResults` | 결과 보기 | Show Results |
| `multiSearch.folderFailed` | 폴더를 읽을 수 없습니다 | The folder could not be read |
| `multiSearch.reason.password` | 암호를 입력하지 않았습니다 | No password was entered |
| `multiSearch.reason.searchFailed` | 검색하지 못했습니다 | Could not be searched |
| `multiSearch.openHit` | {{name}} {{page}}쪽 열기 | Open {{name}}, page {{page}} |

### 15.21 Notes for the implementer

* Korean is natural Korean, not translated English: prefer the 할 수 있습니다 / 해 주세요 register, avoid
  하십시오 and 당신.
* Terminology stays consistent: 주석 (annotation) · 축소판 (thumbnail) · 목차 (outline) ·
  영역 표시 (redaction) · 평면화 (flatten) · 쪽 (counter) vs 페이지 (UI object) · 내보내기 (export) ·
  불러오기 (import).
* Never concatenate strings; always interpolate. Numbers and dates go through `Intl` with the active
  locale. `_other` keys stay even though Korean repeats the singular.
* Undo labels (`undo.*`) are generated per command by the engine and resolve to
  `menu.edit.undo` + " : " + the command's key (e.g. `tool.highlight`), so no extra catalogue is needed.
