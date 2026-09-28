# Stage 5 — compare two documents, autosave / crash recovery (P1-6, P1-8)

Status: landed in the working tree, verified, not committed yet (2026-09-28).

## 1. What landed

* **P1-6 문서 비교.** 도구 → 문서 비교… (the native 도구 menu, the ⋯ overflow menu; no shortcut) opens a
  dialog to pick file B with 대소문자 무시.
  * 비교 opens B with the ordinary `open_document`, **beside** the window's document (never through
    `docStore`). An encrypted B goes through 암호 입력, and the dialog comes back with its state.
  * `compare_documents` runs as a cancellable job with one progress event per page pair. The final
    `done` event carries a `CompareReport` (`JobEvent::Done.compare`).
  * The full-window compare view has a header (변경된 페이지 N · 삽입 N단어 · 삭제 N단어), 이전 / 다음
    변경, 변경만 보기 and 닫기. One scroller holds rows of page pairs, A on the left and B on the right,
    so both sides scroll together by construction. Deleted and replaced words are tinted red on A;
    inserted and replaced words are tinted green on B.
  * B is closed on every exit path: 취소 during the run, the dialog closing, 닫기, Esc, a new compare
    replacing the session, the window's document being replaced or closed, and the window closing.
* **P1-8 자동 저장 / 복구.**
  * `Settings.autosaveSec` (`autosave_sec`, `serde(default = 60)`, so older settings files still load).
    설정 has a 자동 저장 간격 segmented control: 끄기 / 30초 / 1분 / 5분.
  * While the window's document is dirty, a timer calls `write_recovery`. It writes only when
    `docGeneration` moved since the last copy.
  * `write_recovery` writes `$APPDATA/SeePDF/recovery/<uuid>.pdf` + `<uuid>.json`, atomically, from
    the current state (same bytes as a save: appearance streams included, encryption kept). The
    user's file is never touched.
  * 저장, 다른 이름으로 저장 and a clean close (저장 / 저장 안 함 / nothing to save; also window close
    and replacing the document) call `clear_recovery`. A failed write toasts once per document.
  * At launch the main window calls `list_recovery`. When the list is non-empty, the 복구 dialog opens
    with name, 저장 시각, pages and size per row, plus 열기 / 삭제 per row, 모두 삭제 and 나중에.
  * 열기 opens the copy through the normal open path and marks the document as recovered:
    * 저장 becomes 다른 이름으로 저장, suggesting the original file name;
    * the recovery path never enters 최근 항목;
    * saving elsewhere discards the entry.
    A toast says "복구 사본입니다. 다른 이름으로 저장하세요".

## 2. Design decisions

**Words, not characters.**
* A word is a maximal run of characters that are neither Unicode whitespace nor PDFium-generated
  (the synthetic spaces and `\r\n` between text runs). "Whitespace runs are always normalised" falls
  out of the tokenisation.
* `ignoreCase` folds each character with the text layer's `fold` for comparison only. The reported
  text is always the original.

**Diff.**
* The diff is a hand-rolled Myers O(ND). It trims the common prefix and suffix first and interns
  words to integers. No new crate.
* The trace is ≈ D² entries, so D is capped at 2,000. A page pair more different than that becomes
  one `replace` over the untrimmed middle.
* Adjacent delete and insert runs collapse into one `replace`. The lib tests check LCS optimality
  against a brute-force DP on 200 random pairs.

**Rects** come from `TextLayer::range_rects` over the run's code-point span, one per line fragment.

**Job shape.** The job copies compress: one `Lane::Background` command per pair, so Cancel works
between pairs and viewer tiles interleave, plus a finish command that assembles the report. Nothing
is stored on the `OpenDoc`s.

**Pairing is by index.** `pagesA` / `pagesB` pair by position, and the longer list's extra pages
are rows with the other side `null`. No page-insertion alignment is attempted.

**Compare state lives outside the dialog** (`compare/flow.ts`). The 암호 입력 prompt stacks on top
and the dialog host renders only the top entry, so the dialog unmounts mid-run.

**Recovery I/O is split by thread.**
* `recovery::snapshot` runs on the engine thread: `save::serialize`, and it assigns the document's
  recovery uuid the first time. The uuid is kept on the `OpenDoc` for the document's lifetime.
* The file writes run in `spawn_blocking` on the command task, behind a process-wide `IO_LOCK`.
* The PDF is written before the sidecar. `list` deletes a sidecar whose PDF is missing, or that
  does not parse.
* Ids are validated as uuids, so `discard_recovery` cannot name a path outside the directory.

**`clear_recovery` after `close_document`.** The command layer keeps a docId → recovery-id map
(`RecoveryIds`), so the frontend may clear after the engine has already dropped the document.

**`list_recovery` skips copies that belong to documents open right now**, in any window. Those are
live autosaves, not crash leftovers. Added during verification, via
`recovery::open_recovery_ids`.

**Only the main window offers recovery.** A 새 창 must not offer another window's live copies.

## 3. Gates (verification run, 2026-09-28)

| gate | result |
|---|---|
| `cargo check --all-targets` | clean, 0 warnings |
| `cargo build --release --tests && cargo test --release` | 178 passed, 0 failed (incl. 6 in `tests/compare.rs`, 7 in `tests/recovery.rs`) |
| `npm run typecheck` | pass |
| `npx vitest run` | 47 files, 356 tests passed |
| `node scripts/check-i18n.mjs` | ok, ko 600 / en 600 keys (+44 this stage) |
| `npm run build && node scripts/check-bundle-size.mjs` | ok; dist 664.4 kB of 2048; critical path 108.6 kB gz of 120; CompareView, CompareDialog, RecoveryDialog are lazy chunks |
| `node scripts/perf-baseline.mjs --check --skip-app` | ok, every gated row within tolerance (engine + OCR measured; app rows need `tauri dev`) |

No real-app smoke test was run. The evidence is the vitest flows against the mock and the cargo
tests against real PDFium.

## 4. Files changed

Backend:
* New: `engine/compare.rs` (tokeniser, Myers, report assembly, lib tests), `engine/recovery.rs`,
  `commands/compare.rs`, `commands/recovery.rs`, `tests/compare.rs`, `tests/recovery.rs`.
* `ipc/types.rs`: `CompareOptions`, `DiffKind`, `DiffOp`, `ComparePage`, `CompareReport`,
  `RecoveryEntry`, `JobEvent::Done.compare`, `Settings.autosave_sec`, plus a serde shape test.
* `engine/export/job.rs`: `finish_with_compare`. `engine/registry.rs`: `OpenDoc::recovery_id`.
* `app/store.rs`: `recovery_dir`. `lib.rs`: the 5 commands and `RecoveryIds`.
* `commands/{ocr,save}.rs`: `compare: None`.
* `app/menu.rs`: 도구 → 문서 비교… (added during verification).
* `docs/IPC_CONTRACT.md` §7.6b / §7.6c.

Frontend:
* New: `compare/{CompareDialog,CompareView}.tsx`, `compare/{flow,state,model}.ts`, `compare.css`,
  `app/autosave.ts`, `dialogs/RecoveryDialog.tsx`, and tests (`compare.flow.test.tsx`,
  `model.test.ts`, `autosave.test.ts`, `recovery.flow.test.tsx`).
* `App.tsx` (autosave hook, recovery at launch, compare view mount, window-close cleanup),
  `dialogs/flows.ts` (save / close / open hooks), `SettingsDialog.tsx`, `DialogHost.tsx`,
  `dialogState.ts`, `dialogs.css`, `useCommands.ts`, `keys/keymap.ts`, `store/jobStore.ts`,
  `ipc/{types,api,mock,env}.ts`, `i18n/{ko,en}.json`, `test/ipc-samples/settings.json`,
  `docs/UI_SPEC.md`.

## 5. Fixed during verification

* `app/menu.rs`: the native 도구 menu had no 문서 비교 item. It now has `tools.compare`
  ("문서 비교…" / "Compare Documents…", no shortcut), and the label-coverage test passes.
* `commands/recovery.rs` + `engine/recovery.rs`: `list_recovery` offered the live copies of documents
  that are still open. It now filters out every open document's recovery id. There is a new test,
  `open_documents_report_their_recovery_ids`.
* `commands/recovery.rs`: `clear_recovery` now forgets the docId → id entry, so the map no longer
  grows for the whole session.
* `App.tsx`: closing a window during compare mode (or mid-run) leaked document B in the engine,
  because a window's documents are not closed when it is destroyed. The close handler now cancels
  the run and exits compare mode first.
* `engine/compare.rs`: the memory note for the Myers trace said "16 MB of i32"; it is ≈ 32 MB of
  isize.

## 6. Still open

* **A recovered document's title shows the uuid file name** (`<uuid>.pdf`) until it is saved
  elsewhere. 다른 이름으로 저장 suggests the original name.
* **Main-window reload is not covered.** A webview reload (dev, or a renderer crash) loses the
  frontend's recovered-document marks and autosave bookkeeping. The engine side survives.
  `list_recovery` no longer offers the open documents' copies, but the reloaded window starts empty.
* **A compare job ends with a `notFound` error if either document closes mid-job.** The frontend
  toasts 문서를 비교하지 못했습니다. It should be a quiet cancel.
* **More than 2,000 edits on one page** collapse into one `replace` over the whole differing middle.
* **Images and scanned pages have no words.** They compare as empty (OCR first). There is **no pixel
  diff**, so a changed figure or a moved object with the same text shows as 변경 없음.
* Pages pair by index. One inserted page shifts every later pair, so all of them show as changed.
* A copy that was written while the document was dirty stays on disk if undo brings the document
  back to clean. It is cleared on the next save or clean close.
* 저장 안 함 on a recovered document keeps its original recovery entry, so it is offered again on the
  next launch. This is intentional, but it may surprise users.
* No manual QA in the real app yet. Check a compare of two revisions, and a kill -9 while dirty
  followed by a relaunch.
