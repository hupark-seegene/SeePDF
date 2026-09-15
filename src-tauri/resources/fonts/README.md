# Bundled fonts

`bundle.resources` globs this directory, and a glob that matches nothing makes `build.rs`
fail with `GlobPathNotFound` — so this file is committed to keep the glob non-empty (the same
reason `resources/pdfium/LICENSE.pdfium` and `VERSION` are committed).

## `SeePDF-Hangul.ttf` — 487 KB, OFL

The font SeePDF embeds into user PDFs for Korean text boxes, added text and the invisible OCR
layer. Committed on purpose: PDFium's `FPDFText_LoadFont` embeds the **whole** font file with no
subsetting, so a system font would add 6.2 MB (AppleGothic) to 31 MB (AppleSDGothicNeo) to every
document (text spike §5), and AppleGothic's generated `/ToUnicode` maps the space glyph to
U+0009, which turns `한글 테스트` into `한글\t테스트` and breaks search (`ARCHITECTURE.md` D11,
`WORKPLAN.md` §6).

| | |
|---|---|
| source | **Noto Sans KR** (variable), instanced at `wght 400`, SIL OFL 1.1 |
| built by | `cargo run --release --example build_hangul_font -- <path to NotoSansKR-Variable.ttf>` |
| coverage | 3,126 glyphs: ASCII, Latin-1, Hangul Jamo + Compatibility Jamo, the 2,350 KS X 1001 syllables, CJK and general punctuation, fullwidth forms, ₩ and € |
| names | family `SeePDF Hangul`, PostScript `SeePDF-Hangul` — what `PageObject.fontName` reports and what `TextEditProbe.substituteFont` promises |
| licence | `OFL.txt`, next to it — it carries **Adobe's** copyright, because Noto Sans KR is built from Source Han Sans; that is the licence text Google ships with it |
| not covered | U+2252, U+3130, U+318F (no glyph in the source) and U+3164, U+FF5E (dropped: they share a glyph with U+0020 / U+301C) |

Two properties the rest of the project depends on, both asserted by
`engine::fonts::tests::bundled_font_covers_the_ksx1001_repertoire` and by the builder itself:

* every KS X 1001 syllable resolves to a real glyph — PDFium silently drops glyphs it cannot
  find, so a missing syllable would vanish from an OCR layer with no error anywhere;
* **no two code points share a glyph.** PDFium generates `/ToUnicode` by reverse-mapping the
  font's own `cmap`, so a many-to-one `cmap` lets it pick the wrong code point — that is the
  AppleGothic space→TAB bug. The builder rebuilds the `cmap` with one code point per glyph and
  refuses to write a font that breaks the rule.

`subsetter` deletes the `cmap` table (its target is PDF writers that supply their own CMap),
which PDFium needs, so `build_hangul_font.rs` subsets the outlines with `subsetter` and then
writes a fresh sfnt container with its own `cmap`, a renamed `name` table and the source's
`OS/2`. See the module docs in that example.

## Rebuilding

Any OFL Korean TTF works as the source; a static Regular face is used as-is and a variable font
is instanced at `wght 400` (Noto Sans KR's default master is `wght 100`, which would ship a
hairline). NanumGothic-Regular also builds (1.22 MB) but has no conjoining Jamo and is missing
57 accented Latin-1 characters, so Noto is what ships.

After rebuilding, re-run `cargo run --release --example gen_fixtures` — `fixtures/gen/korean-300dpi.pdf`
embeds this file.
