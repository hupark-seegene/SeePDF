# v0.3 — OCR package notes (pkg7-ocr: U1, O6, O1, O2, O3, O5)

Decisions and measurements behind the v0.3 OCR items; the contract is `IPC_CONTRACT.md` §7.9 ("v0.3
pkg7-ocr"), the UI `UI_SPEC.md` §5 / §10 / §15.11, the status `FEATURES.md` "v0.3 status".

## Release languages (O1)

`scripts/prepare-ocr.mjs --langs jpn,chi_sim` stages Japanese and Simplified Chinese traineddata
(`4.0.0_best_int`, gzipped): **jpn 1.94 MB, chi_sim 1.64 MB** (measured from jsdelivr, 2026-09-29) — the OCR
assets go from 8.14 MB to 11.72 MB. The release **keeps `kor` + `eng`**:

* on macOS, Apple Vision reads `ja-JP` and `zh-Hans` on every supported Mac (13+) at no bundle cost, and 자동
  already picks Vision there;
* on Windows, Windows OCR reads them where the language pack is installed (O3);
* the primary user's documents are Korean; +3.6 MB (+44 % of the OCR assets) for a Tesseract-only fallback is not
  worth it yet.

`public/ocr/tessdata/languages.json` lists what is staged; the frontend adds it to the tesseract languages of
`ocr_capabilities` (only the frontend can see its own assets). A language staged earlier and not requested now is
removed, so the bundle is exactly what the flags say.

## Orientation detection (O2)

The spec asked for "recognise at 0/90/180/270 and pick the highest mean confidence (15 % margin)". That is what
the tesseract path does. **It does not work with Apple Vision**: on the sideways Korean fixture Vision returned
the same 24 words at 100 % confidence for all four turns — it reads rotated text by itself (and its axis-aligned
boxes of rotated lines would then put the layer in the wrong place). So Vision detection asks for each line's
reading direction instead (the `bottomLeft → bottomRight` angle of the observation's quadrilateral), votes by
characters and uses the same pick rule (≥ 3 words, 1.15 × runner-up and as-is): one Vision read at 100 DPI
instead of four. Sideways fixture: 100 % of characters at 90°, 0 elsewhere. Windows OCR reports no confidence at
all, so a Windows run detects with tesseract (the pool is created for detection only).

A failed detection is "leave the page as it is", never a failed page.

## Glyphless font (O5)

* The font is built in code (`build_font`, ~0.9 KB, 452 B flate-compressed in the PDF): `.notdef` + a 1 em box +
  a ½ em box. Box glyphs rather than empty ones: never painted (render mode 3), but they give every character a
  real bbox for selection and search highlights.
* **One font per document** (verification round 1): CID = the BMP code point, an identity `/ToUnicode` (one
  `bfrange` per high byte, surrogates skipped) and a 65 536-entry `/CIDToGIDMap`, loaded with
  `FPDFText_LoadCidType2Font` the first time a document needs it and cached on `OpenDoc` (dropped by
  `registry::replace` — undo, redo, save — before the old document closes). The first version loaded a font per
  `ocr_apply` with only that call's characters; the OCR sheet applies **one page per call**, so a 30-page scan
  embedded 30 fonts (~27 % of the subset's growth) and a 120-page one ~93 %.
* Text is written with `FPDFText_SetCharcodes` (the CIDs directly). `FPDFText_SetText` would find each code by
  `CPDF_ToUnicodeMap::ReverseLookup`, a linear scan — over a BMP-wide map, milliseconds per character.
* `/W` gotcha: PDFium writes `/W` from the map as `c_first c_last w` for runs of equal widths, but a CID whose
  width differs from its successor's opens a `c [w1 w2 …]` list that swallows every following CID. With CID 0 as
  a 1 em `.notdef` in front of ½ em Latin, `/W` was a flat list of 65 536 numbers (323 KB). The half-width
  classes are therefore ranges of ≥ 2 CIDs and CID 0 is half-width too: `/W` is ten ranges (167 B).
* Measured (`tests/ocr.rs`): one Korean page grows the saved file by **4 173 B** vs **262 880 B** with the
  Hangul subset (1.6 %; the identity `/ToUnicode` is 1.7 KB compressed); **32 pages applied one call each**
  grow it by **25 074 B** vs **283 668 B** (8.8 %) — one embedded font, the rest is the pages' own text
  (`ocr_glyphless_font_is_embedded_once_across_per_page_calls`). Beyond that the growth is the layer's content
  streams, which cost the same with either font.
* Characters outside the BMP (and U+0000) become U+FFFD.

## Windows OCR (O3)

`windows` 0.61 — the version Tauri already links on Windows, so `Cargo.lock` gains a feature set, not a crate.
Bgra8 premultiplied `SoftwareBitmap` (the format every OcrEngine revision accepts), `RecognizeAsync().get()` on
a tokio blocking thread (windows-core joins the MTA itself). Pages beyond `MaxImageDimension` are read at an
integer fraction and the boxes scaled back. Checked here only by `cargo clippy --target x86_64-pc-windows-msvc`
(llvm-rc stand-in) and the cross-platform mapping tests; the real call runs on the Windows CI runner
(`tests/ocr.rs ocr_windows_reads_an_english_fixture`, skipped when the en-US recogniser is missing).

One recogniser per call (verification round 2): `OcrEngine::TryCreateFromLanguage` takes one language, and the
ko / ja / zh recognisers also read Latin text. So Windows OCR counts as reading one non-English language plus
English (`engine.ts` `windowsLanguages`): 자동 takes it only when that covers the selection (한국어 + English,
English + 日本語, one language), otherwise the round-1 scoring decides with that reduced coverage — 한국어 +
日本語 goes to Tesseract when jpn traineddata is staged; when neither engine reads both, Windows reads Korean,
日本語 is shown unpressed and `ocr.engine.windowsOneLanguage` says why. With Windows OCR chosen explicitly a
newly pressed non-English chip replaces the other one. `winocr::pick_language` prefers the non-English
requested language (`eng+jpn` → `ja`). Running one pass per selected CJK language and merging was not done:
overlapping reads of the same ink would need de-duplication.

## Scanned-document banner (O6)

Sampling is `ocr_page_status` of the first 10 pages when the window is idle after open (`requestIdleCallback`,
1.5 s timeout) — one engine call on the background lane. "No page has text" uses the engine's own threshold
(< 16 extractable characters), so a stamped scan still counts as a scan. Dismissal is per path (per document id
for an unsaved one), in memory for the session. The banner is its own lazy chunk (with its CSS), mounted by the
viewer and shared with the 검색 panel's hint; the entry chunk does not grow.
