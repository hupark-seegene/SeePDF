# v0.3 pkg5 — app shell, release, diagnostics

Items H1, H10, H12, H3, H2, H4, V7, H11, U4, O4, H13, H14 (backlog order). Feature rows: `FEATURES.md` ▸ v0.3
status. Contract: `IPC_CONTRACT.md` §2 (`engineCrashed`, `open_document` `detail`), §7.6c (owners), §8
(`engine-crashed`, `theme-changed`), §11b. UI: `UI_SPEC.md` §1, §2, §8, §10, §13, §15.20c.

## Decisions

1. **O4 — macOS minimum 13.0, no fallback core.** 0 MB, versus +3.9 MB for the non-SIMD tesseract core and a
   SIMD probe in the worker. One nuance found while checking the premise: WASM SIMD arrived with WebKit 16.4
   (macOS 13.3, and Safari 16.4 updates on 12.x), so 13.0–13.2 without updates still lack it — but on every
   13.x Apple Vision is the default OCR engine (P1-11) and tesseract is only the fallback, so OCR works on
   the whole supported range. A user who needs 12.x: pass `--with-fallback-core` in release.yml and lower the
   minimum again; `check-release-config.mjs` will then need its O4 assertion relaxed.
2. **H2 — liveness by lock, not by pid.** The spec asked for "pid + start timestamp, skip when the pid is
   alive". Checking an arbitrary pid portably needs `kill(pid, 0)` / `OpenProcess`, and a recycled pid would
   hide a dead session's copies forever. Instead the writing process holds an exclusive `File::try_lock`
   (std, no new dependency) on `.owner-<pid>-<started>.lock` in the recovery directory; the sidecar names that
   owner. A lock that can be taken means the owner is gone (the OS released it), whatever happened to the
   pid. The pid and start time are still in the sidecar and the lock name, for diagnostics.
3. **H3 — "읽기 전용" means the PDF's own permissions.** `DocInfo` has no file-writability flag (that is only
   known when saving, `check_writable`), so the badge shows when `permissions.modify` is false — the case in
   which editing commands are refused. A read-only *file* still gets the existing Save-As fallback on 저장.
4. **H4 — which document is "offending".** Commands do not carry a doc id, so `EngineState::doc` /
   `doc_mut` record every document the running command looked up (thread-local, reset per command); all of
   them are closed after a panic. The reply channel gets `engineCrashed` from inside the command wrapper
   (`dispatch_reply`), then the unwind resumes so the loop does the closing; fire-and-forget commands are
   covered by the loop's own `catch_unwind`. `registry::close` itself runs under `catch_unwind`; if it panics
   too, the document is forgotten without PDFium (a leak beats a crash).
5. **H10 — the clipboard.** A native-menu click carries no user activation, and WKWebView may refuse
   `navigator.clipboard.writeText` then. 문제 보고 therefore also writes `problem-report.txt` next to the logs
   and reveals it, and the toast says which of the two happened. No clipboard plugin was added.
6. **H10 — the log writer is synchronous** (`RollingFileAppender` as the `MakeWriter`), not
   `tracing_appender::non_blocking`: a `std::process::exit(1)` after a startup failure, or a native crash,
   would otherwise lose the last lines — the ones that matter.
7. **H11 — notices size.** 583 components (every platform's crates: the resolver's superset), ~400 kB after
   printing identical texts once and the Apache-2.0 text once (864 kB without). Generated, gitignored, run in
   release.yml (`--check`) and ci.yml. A dev build without it shows the PDFium and font licences.
8. **H12 — the offline installer** is a second `tauri build --bundles nsis` with a config override
   (`offlineInstaller`, no updater artifacts); it overwrites `bundle/nsis`, so the workflow sets the main
   installer (and its `.sig`) aside first. `release-latest-json.mjs` never lists it.
9. **H14 — the clamp only shrinks.** A window that fits 90 % of its monitor's work area is left exactly where
   window-state put it; a larger one (first run on a small laptop, or a size restored from a bigger monitor)
   is shrunk and centred. Maximised / full-screen windows are skipped.
10. **V7 — ⇧H** latches the hand tool from the keyboard (H is the highlighter; Space stays the momentary
    hold). The native 보기 ▸ 손 도구 item has no accelerator, so it never shadows the canvas-only key.

## Build notes

* Windows clippy on this Mac: `RC_x86_64_pc_windows_msvc` → a shim that answers embed-resource's `/?` probe as
  llvm-rc (`OVERVIEW: LLVM Resource Converter` + `no-preprocess`) and creates an empty `.res`; plus an empty
  `src-tauri/resources/pdfium/pdfium-x86_64-pc-windows-msvc.dll` (gitignored) so the Windows resource glob
  matches. `cargo clippy --target x86_64-pc-windows-msvc -- -D warnings` then builds the single-instance
  plugin and passes.
* `panic = "unwind"` also removes the profile mismatch STAGE1B_NOTES §5.3 worked around (the lib is still
  `rlib` only).

## Not done here (owned elsewhere)

* H3's Thumbnails `aria-current` (pkg2), StampDialog `stamp.error.noImage` (T1) and SecurityDialog
  `dialog.security.empty` (pkg3).
* V7's 스냅샷 segment (V1, pkg6): one more row in `READ_TOOLS` in `StatusBar.tsx`.
* H2's "through H8's already-open check": the hand-off calls `app::files::push_open`, so whatever H8 adds
  there applies.
