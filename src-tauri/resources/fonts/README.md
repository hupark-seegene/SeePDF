# Bundled fonts

`bundle.resources` globs this directory, and a glob that matches nothing makes `build.rs`
fail with `GlobPathNotFound` — so this file is committed to keep the glob non-empty (the same
reason `resources/pdfium/LICENSE.pdfium` and `VERSION` are committed).

Stage 1 (b) drops the real payload here:

* `SeePDF-Hangul.ttf` — a Hangul subset (Noto Sans KR or Pretendard, both OFL, ~1–1.5 MB),
  built once by `src-tauri/examples/build_hangul_font.rs` and committed. It is embedded into
  user PDFs for text boxes, added text and the OCR layer. Never a system font: PDFium embeds
  the *whole* file (AppleGothic +6.2 MB, AppleSDGothicNeo +31 MB) and AppleGothic's generated
  `/ToUnicode` maps space → TAB, which breaks search (text spike §5, ARCHITECTURE D11).
* the font's licence file, next to it.

`src-tauri/examples/gen_fixtures.rs` also waits for `SeePDF-Hangul.ttf` before it can build
`fixtures/gen/korean-300dpi.pdf`.
