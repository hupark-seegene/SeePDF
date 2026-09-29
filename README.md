# SeePDF

A lightweight, fast, cross-platform **PDF editor with offline OCR**, built for Korean users first.

SeePDF opens a 500-page scan as quickly as a viewer does, and then lets you actually change it:
annotate, fill forms, reorder pages, edit text and images, redact, recognise Korean text and save
— all locally, with no account, no upload and no network access at any point.

* **Stack:** [Tauri v2](https://tauri.app) (Rust) + React 19 / TypeScript / Vite 8
* **PDF engine:** [PDFium](https://pdfium.googlesource.com/pdfium/) via `pdfium-render`, on one
  dedicated engine thread (PDFium is not thread-safe)
* **OCR:** [tesseract.js](https://github.com/naptha/tesseract.js) with the `kor` + `eng`
  `4.0.0_best_int` models, bundled — OCR works with the network cable pulled; on macOS 13+ Apple
  Vision is used by default (Hangul CER 0 % vs 1.16 %, ~3× faster)
* **Platforms:** macOS 13+ (arm64 + x86_64) and Windows 10/11 (x86_64)
* **Size:** a 12.5 MB Windows installer / 14 MB macOS dmg, no Electron, no Chromium copy — the OS
  webview does the drawing

## Download

The current version is **0.3.0** — what changed in each release is in [CHANGELOG.md](CHANGELOG.md) (Korean
first), and the same notes are on each GitHub release. Installers are published on
[GitHub Releases](https://github.com/hupark-seegene/SeePDF/releases/latest). Which one to pick:

| File | For |
|---|---|
| `SeePDF_<version>_x64-setup.exe` | **Windows — recommended.** Per-user install, Korean or English (the installer asks). Installs WebView2 if it is missing, which needs the internet once (Windows 11 and most Windows 10 PCs already have it). |
| `SeePDF_<version>_x64-offline-setup.exe` | Windows PCs **without internet access** (firewalled or air-gapped) that may lack WebView2: carries the whole WebView2 runtime installer, so it is much larger. |
| `SeePDF_<version>_x64_ko-KR.msi` / `_en-US.msi` | Windows managed deployment (Intune, GPO), Korean or English. |
| `SeePDF_<version>_aarch64.dmg` | Mac with Apple silicon (M1 and later), macOS 13 or newer. |
| `SeePDF_<version>_x64.dmg` | Intel Mac, macOS 13 or newer (new in 0.3). |

The builds are not code-signed yet: Windows SmartScreen shows "추가 정보 → 실행", and macOS needs 시스템 설정
→ 개인정보 보호 및 보안 → "그래도 열기". Installed copies check for updates at launch (설정 → 일반) once signed update
bundles are published (see `.github/workflows/release.yml`); until then, install a new version over the old one —
settings, signatures, stamps and recent files are kept.

If something goes wrong, ⋯ ▸ 문제 보고… (macOS: 도움말 ▸ 문제 보고…) copies the version, OS and the recent log
to the clipboard and opens the log folder (`~/Library/Logs/com.seepdf.desktop` on macOS,
`%LOCALAPPDATA%\com.seepdf.desktop\logs` on Windows; one file a day, a week kept).

---

## Features

### Viewing
* Continuous / single / two-page layouts, 25–400 % zoom, fit-page, fit-width, actual size,
  ⌘± / ctrl+wheel / pinch, non-destructive rotate (⌘L / ⌘R)
* Virtualised scrolling with 512 px tiles: only pages within ±1.5 screens are mounted, so a fling
  through 500 pages costs no React work per frame
* Thumbnail rail, outline sidebar (jumps to the heading, not just the page), night mode
* Text selection and copy with real line breaks, streamed search with context, ⌘G / ⇧⌘G

### Annotating
형광펜 · 밑줄 · 취소선 · 물결선 · 펜(ink) + 지우개 · 사각형 · 타원 · 선 · 화살표 · 메모 ·
텍스트 상자 (Korean included) · 도장/서명 이미지 — with colour, opacity, border width, author and
note properties, an annotation list sidebar, and undo/redo for every one of them.

Annotations are written as real PDF annotations with generated `/AP` streams, so macOS Preview and
Acrobat show exactly what SeePDF showed.

### Pages, content and forms
* Page organizer: drag to reorder, delete, rotate, duplicate, insert blank, insert from another
  file, extract, merge, split — one drag is one undo step
* Edit existing text (with an honest refusal when the font cannot draw the new string), add text
  boxes with the bundled Hangul font, insert / move / resize / replace images
* AcroForm filling: text, checkbox, radio, combo, list, with field highlighting
* **Redaction with true content removal**, verified after the fact: if the marked string can still
  be extracted from the result, the whole operation is rolled back and reported
* 보안: 암호 설정 (AES-256, 열기/권한 암호 + 권한) / 암호 제거, 문서 정보(메타데이터) 편집 및 메타데이터 제거
* 워터마크 / 머리글·바닥글: text (with `{{page}}` / `{{total}}` / `{{date}}` / `{{filename}}`) or an
  image, 9 anchors, rotation, opacity, page range — one undo step, correct on rotated pages
* 압축: downsample images to 300 / 150 / 96 DPI on a scratch copy, show the measured before/after
  size, then apply (one undo step) or discard
* 문단 편집: click a paragraph, retype it, and the text re-flows in its box; the content below moves
  down or up to make room, with fit / overlap options when the page is full (one undo step)
* 문서 비교: word-level text diff of two PDFs page by page, shown side by side with deletions marked
  on the left and insertions on the right, 이전/다음 변경 navigation and 변경만 보기
* 자동 저장 / 복구: while a document has unsaved changes, a recovery copy is kept (every 30 s / 1 min /
  5 min, or off) without touching your file; after a crash SeePDF offers to reopen it

### Structure and review (v0.2)
* 목차 편집 (add / rename / indent / drag / set destination), 링크 만들기 (page or web), 페이지 레이블
  (i, ii, … then 1, 2, …) shown in the page box, thumbnails and 목차
* 페이지 자르기 (with automatic margin detection) and 페이지 크기 변경 (A4 / Letter / A3 / custom, scale or
  centre), Bates numbering (`{{bates}}`, prefix / digits / start)
* 주석 답글 (threads that survive save and reopen in other viewers), 주석 목록 내보내기 (TXT / CSV / Markdown)
* 여러 파일에서 검색 (files or a whole folder), 분할 보기 (two independently scrolled panes of one document),
  읽어 주기 (the OS voice, offline), 업데이트 확인

### New in 0.3
* **Redaction you can trust on real documents:** one word removes only that word, a mark over a scan
  blacks out those pixels in the image itself, 검색해서 표시 finds keywords and Korean personal data
  (주민등록번호, 전화번호, 이메일, 계좌번호) on every page, and text inside groups can be ungrouped and edited
* **Signed and restricted PDFs handled properly:** 서명됨 detection with incremental save (signatures stay
  valid), PDF permission flags enforced with a 제한됨 badge and unlock by the permissions password, structure
  edits on encrypted files, 문서 정리 (JavaScript, attachments, hidden data), a 첨부 파일 panel
* **Pages and forms:** 이미지로 PDF 만들기 (files, drops, clipboard), merge / insert that keep bookmarks, labels
  and fields, drag pages between windows, split by bookmarks, outline from headings, **form field
  authoring** plus CSV / XFDF form data
* **Annotations:** real line / polygon / cloud / callout annotations and a measure tool, partial ink
  erasing, pen pressure and palm rejection, a quick popover, 내 도장 and dynamic stamps, saved image
  signatures, the author name from 설정, per-annotation 인쇄 안 함
* **OCR:** Japanese and Chinese (Apple Vision), automatic page-rotation detection, Windows' built-in OCR,
  a scanned-document banner, smaller OCR'd files
* **Output:** 여러 파일 처리 (watermark / Bates, compress, encrypt, flatten, images over many files), 모아찍기 /
  소책자 / 흑백 printing, 실제 크기 printing laid out for the chosen paper, image extraction, long-image and
  multi-page TIFF export, text-flow DOCX / HWPX / HTML / Markdown, pixel 문서 비교, better 압축
* **Viewing and the app:** 스냅샷, context menus, a draggable split divider, 두 쪽 (표지 따로), auto web
  links, sentence-by-sentence 읽어 주기, a latched hand tool, screen-reader text and caret mode (F7), a
  single instance on Windows, crash-proof engine, 문제 보고 / 로그 폴더 / 오픈 소스 라이선스, backups on save

### OCR
Current page / all pages / a range, `kor+eng`, at 200–400 DPI, with progress and cancel. The
recognised words are written as an **invisible text layer** over the scan, so the page still looks
like a scan but is selectable, searchable and copyable — in SeePDF and in every other viewer.

### Output
Save / Save As (atomic: temp file → fsync → verify by reopening → rename, with the original left
byte-identical if the process dies), export pages as PNG / JPEG, export text, export a flattened
PDF, print.

### Shell
Korean-first UI with a full English translation, light / dark / system themes, the complete
macOS and Windows shortcut tables, a native macOS menu bar, recent files with restored reading
position, and a welcome screen.

---

## Running it

### Requirements

| | macOS | Windows |
|---|---|---|
| OS | 13.0 Ventura or newer (the OCR core needs WebKit's WASM SIMD) | Windows 10 1809 or newer |
| Toolchain | Xcode Command Line Tools | MSVC build tools + WebView2 (preinstalled on Win 11) |
| Rust | 1.90+ (`rustup`) | 1.90+ (`rustup`), `x86_64-pc-windows-msvc` |
| Node | 24+, npm 11+ | 24+, npm 11+ |

### First run

```sh
git clone https://github.com/hupark-seegene/SeePDF.git
cd SeePDF
npm install          # postinstall fetches libpdfium for this host and the OCR assets
npm run tauri dev
```

`npm install` runs two download steps, both of which need the network **once**:

* `scripts/fetch-pdfium.mjs` → `src-tauri/resources/pdfium/` (the binaries are gitignored).
  `npm run fetch:pdfium -- --all` fetches every target; `PDFIUM_TARGET=<triple>` picks one.
* `scripts/prepare-ocr.mjs` → `public/ocr/` (tesseract.js runtime + `kor`/`eng` traineddata,
  8.4 MB, gitignored). After this the app never touches the network again.

Open a file on launch with `npm run tauri dev -- -- -- /path/to/file.pdf`.

### Building a bundle

```sh
npm run tauri build                      # .dmg + .app  /  .msi + NSIS .exe
npm run tauri build -- --debug           # unoptimised, with devtools
npm run tauri build -- --target x86_64-apple-darwin
```

On Windows, run the same commands from a *Developer Command Prompt* (or any shell where `link.exe`
is on `PATH`) and set `PDFIUM_TARGET=x86_64-pc-windows-msvc` before `npm ci` if you are
cross-preparing resources.

### Checks

```sh
npm run typecheck                    # tsc --noEmit
npx vitest run                       # ~1040 frontend tests (135 files)
node scripts/check-i18n.mjs --strict # ko/en key + placeholder parity, no dead keys (CI gate)
npm run build && node scripts/check-bundle-size.mjs   # critical-path budget

cd src-tauri
cargo build --release --tests && cargo test --release # ~610 engine tests (41 suites) against real fixtures
cargo fmt --check && cargo clippy --all-targets -- -D warnings
```

Useful tools:

```sh
# what is actually inside a saved file: pages, annotations, form values, extracted text
cd src-tauri && cargo run --release --example inspect_pdf -- ../fixtures/out/stage2/annotated.pdf

cargo run --release --example gen_fixtures        # regenerate fixtures/gen/**
node scripts/perf-baseline.mjs --write            # docs/perf/baseline.md
```

---

## How it is put together

```
src/                     React 19 frontend
  ipc/                   the wire contract (docs/IPC_CONTRACT.md), one file per side
  viewer/                virtualised scroller, tiles, text layer, search
  annot/ tools/ forms/   annotation overlay, tool controller, AcroForm inputs
  organize/ dialogs/     page organizer, every sheet
  ocr/                   tesseract.js worker pool, Hangul normalisation
  store/                 zustand slices (doc, view, app, annots, pages, jobs)
src-tauri/               Rust backend
  engine/                THE pdfium thread: registry, render, text, annot, pages, ocr, save
  protocol/              the seepdf:// scheme that carries every pixel
  commands/              every #[tauri::command]
  ipc/                   the Rust half of the contract
docs/                    ARCHITECTURE · IPC_CONTRACT · UI_SPEC · FEATURES · WORKPLAN · stage notes
```

Two rules the code enforces rather than documents:

1. **Every PDFium call happens on the engine thread.** The `thread_safe` feature of
   `pdfium-render` is deliberately off, which makes its types `!Send`, so the compiler rejects any
   attempt to touch a document from a command thread.
2. **`registry::mutate` is the only `&mut` path into a document.** It takes the undo snapshot,
   flushes the page LRU, bumps the generation, invalidates the caches and emits `doc-changed`.
   One user action = one `mutate` = one generation = one undo step.

`docs/ARCHITECTURE.md` has the long version, including why pixels travel over a custom URL scheme
instead of the IPC channel and how the tile budget works.

---

## Licences

SeePDF's own source is in this repository. The things it ships with are not ours:

| Component | Licence | Where |
|---|---|---|
| **PDFium** (build 155.0.8057) | BSD-3-Clause (Chromium/PDFium) + MIT for the `pdfium-binaries` packaging by Benoit Blanchon | `src-tauri/resources/pdfium/LICENSE.pdfium`, binaries downloaded at install time |
| `pdfium-render` | MIT / Apache-2.0, used through a 5-line additive fork (`docs/pdfium-patch.diff`, vendored in `vendor/pdfium-render`) | `vendor/pdfium-render/LICENSE*` |
| **SeePDF-Hangul.ttf** — a subset of **Noto Sans KR**, instanced at `wght 400` | SIL Open Font License 1.1 (the file carries Adobe's copyright: Noto Sans KR derives from Source Han Sans) | `src-tauri/resources/fonts/OFL.txt`, rationale in `resources/fonts/README.md` |
| **tesseract.js** 7.0 runtime + **tesseract.js-core** (Tesseract OCR as WebAssembly) | Apache License 2.0 | `public/ocr/LICENSE.txt` |
| **`kor` / `eng` traineddata** (`tessdata_best` 4.0.0_best_int) | Apache License 2.0 | `public/ocr/LICENSE.txt` |
| Tauri, React, zustand, lucide-react, ttf-parser, image, png, … | MIT / Apache-2.0 | `package.json`, `src-tauri/Cargo.toml` |

Every release bundles `THIRD_PARTY_NOTICES.txt` — the licence of each Rust crate and npm package in the
build plus the files above, generated by `scripts/gen-notices.mjs` — and shows it in the app under
SeePDF 정보 ▸ 오픈 소스 라이선스.

The bundled font is embedded **into the PDFs you create** whenever you type Korean into a text box
or apply OCR. The OFL permits that; the licence text travels in the bundle
(`resources/fonts/OFL.txt`) and applies to the embedded subset.

Fonts and OCR data are downloaded at install time and are not committed, so a clone of this
repository contains no third-party binaries.
