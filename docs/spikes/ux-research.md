# SeePDF — Product & UX Research (spike)

**Status:** research complete, no code. **Date:** 2026-09-15. **Author:** UX research agent.
**Audience:** the implementation agents building the SeePDF v1 shell, toolbar, sidebar, canvas chrome and i18n layer.

This document is the source of truth for **what SeePDF v1 ships** and **what it looks like**. Sections 1–2 justify the
scope; sections 3–8 are the buildable spec. If you only read one thing, read §8 (v1 scope P0/P1/P2) and §4 (UI spec).

**Contents:** [0 Method](#0-method--confidence) · [1 Landscape](#1-competitive-landscape--one-paragraph-each) ·
[2 Feature matrix](#2-feature-matrix) · [3 Positioning](#3-what-v1-must-be-in-one-sentence) ·
[4 UI spec](#4-ui-specification--seepdf-v1) · [5 i18n catalogue](#5-i18n-string-catalogue-v1-complete) ·
[6 Design tokens](#6-design-tokens) · [7 Performance targets](#7-performance-expectations--targets) ·
[8 Scope P0/P1/P2](#8-v1-scope--p0--p1--p2)

---

## 0. Method & confidence

- Feature data comes from hands-on knowledge of the tools plus vendor documentation checked 2026-09 for the Korean
  products (unidocs ezPDF, Hancom 한/PDF, ESTsoft 알PDF), where my prior knowledge was thinnest.
- Numbers in §7 (performance) are **targets derived from observed behaviour of the reference tools**, not vendor specs.
  Where I state a measured figure I say which machine class it is for (M-series Mac, 16 GB).
- Confidence: high on Acrobat / Preview / PDF Expert / Foxit (long familiarity); medium-high on Nitro / UPDF / PDFgear;
  medium on the Korean three — their feature sets move fast and are partly bundled with office suites.

Sources consulted for the Korean tools and the 2026 state of the mid-market:
[ezPDF Editor 3.0](https://www.ezpdf.co.kr/editor3/main.do) ·
[ezPDF Editor 3 manual (PDF)](http://www.ezpdf.co.kr/data/ezPDFEditor3_manual.pdf) ·
[한컴오피스 한/PDF 도움말](https://help.hancom.com/hoffice110/ko-KR/Hpdf/index.htm) ·
[한컴 OCR](https://developer.hancom.com/hancomocr) ·
[알PDF OCR 설치 안내](https://altools.co.kr/service/FAQ?no=257) ·
[Best PDF editors 2026 comparison](https://dupple.com/learn/best-pdf-editors)

---

## 1. Competitive landscape — one paragraph each

| Tool | Platform | Price (2026) | UI shell | The one thing it is known for |
|---|---|---|---|---|
| **Adobe Acrobat Pro** | mac/win/web | ~₩25,000/mo | Ribbon-ish "Tools" rail + right task pane | Completeness. Everything exists, nothing is fast. Cold open of a 14-page PDF still costs 2–4 s. |
| **PDF Expert (Readdle)** | mac/iOS | $79.99/yr | Native macOS: single toolbar, left sidebar, mode switcher (Annotate/Edit/Organize/Fill&Sign) | Speed + feel. The gold standard for "native Mac". Opens a 500-page doc in well under a second. |
| **Foxit PDF Editor** | mac/win | ~$159 perpetual / $10.99 mo | Office ribbon (tabs: Home/Edit/Comment/Organize/Form/Protect) | Acrobat parity at half price; heavy but complete. Strong forms + redaction. |
| **Nitro PDF Pro** | win (mac via Nitro PDF Pro for Mac) | ~$180 perpetual | Office ribbon | Enterprise Windows deployments, Office-style muscle memory, batch + e-sign. |
| **PDFgear** | mac/win/iOS | Free | Compact toolbar + tool grid home screen | Free, surprisingly complete (edit/OCR/convert/AI chat), but no redaction, no real security depth. |
| **UPDF** | mac/win/iOS/Android | $49.99/yr, $79.99 lifetime | Left vertical icon rail + contextual top bar | Cross-device + cheap perpetual; polished; AI summarise. Forms/redaction shallower than Acrobat. |
| **Preview.app** | mac | free/bundled | Markup toolbar toggle, thumbnail sidebar | The baseline every Mac user already has. Fast, annotate + reorder + sign; no text editing, no OCR, no forms authoring. |
| **ezPDF Editor 3.0** (unidocs) | win | ~₩33,000 perpetual (개인) | Ribbon tabs: 홈/편집/주석/양식/보안/변환 | The Korean office default for 관공서 forms. Strong AcroForm fill/author, 한글 OCR, 공인/전자 서명 flow. |
| **한컴오피스 한/PDF** | win (한컴오피스 번들) | bundled with 한컴오피스 | 한컴 ribbon | PDF ⇄ hwp/hwpx/docx conversion with 한컴 OCR. Viewer-plus-convert, not a full editor: annotation only (형광펜/밑줄/취소선/도형/스티커노트), no text editing. |
| **알PDF** (ESTsoft) | win/mac/iOS/Android | free (개인), 유료 기업 | 알툴즈-style toolbar | Free for personal use, wide Korean install base, OCR as a separate downloadable module, AI 요약 in 2025+. |

### What this tells us

1. **The Korean mid-market is served by Windows-first, ribbon-heavy, visually dated software.** ezPDF and 알PDF are the
   incumbents; neither feels native on macOS (알PDF's Mac build is a port). 한/PDF is not an editor at all.
   *A fast, genuinely native, Korean-first mac+win editor is an open position.*
2. **Everyone below Acrobat drops the same four things:** deep redaction, PDF/A + preflight, Bates numbering,
   and real form *authoring* (as opposed to filling). We should drop three of those and keep **redaction**, because
   redaction is the one that Korean business users (개인정보 마스킹) actually ask for and PDFgear's lack of it is the
   most cited complaint.
3. **Text editing is now table stakes**, even in free tools (PDFgear, 알PDF). It is also the single hardest thing to do
   well with PDFium. Plan: v1 ships *add* text now and *edit existing text runs* later (see §8.1), not reflowing layout.
4. **"Open instantly" is the differentiator people can feel in the first 10 seconds.** PDF Expert won its market on
   this. It is the one metric to defend obsessively (§7).

---

## 2. Feature matrix

Legend: ● full · ◐ partial / limited / paid add-on · ○ absent · — not applicable

Columns: **AcrP** Acrobat Pro · **PDFx** PDF Expert · **Fox** Foxit · **Nit** Nitro · **PDFg** PDFgear · **UPDF** ·
**Prev** Preview.app · **ezPDF** ezPDF Editor 3 · **한PDF** 한컴 한/PDF · **알PDF** · **v1** SeePDF v1 target ·
**later** SeePDF post-v1.

### 2.1 Viewing

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Continuous scroll | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Single page (paged) | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Two-page / spread | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Cover page in spread | ● | ● | ● | ● | ◐ | ● | ● | ◐ | ◐ | ◐ | ○ | ● |
| RTL / book direction | ● | ◐ | ● | ◐ | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Zoom: fit page / width / actual | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Zoom to selection (marquee) | ● | ◐ | ● | ● | ○ | ◐ | ○ | ● | ◐ | ◐ | ○ | ● |
| Pinch / trackpad zoom | ● | ● | ● | ◐ | ● | ● | ● | ◐ | ◐ | ◐ | **●** | |
| Thumbnails sidebar | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Outline / bookmarks sidebar | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Edit/create bookmarks | ● | ● | ● | ● | ○ | ● | ○ | ● | ○ | ◐ | ○ | ● |
| Annotation list sidebar | ● | ● | ● | ● | ◐ | ● | ○ | ● | ◐ | ◐ | **●** | |
| Attachments / layers panel | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ◐ |
| Text search (in doc) | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Search results list with context | ● | ● | ● | ● | ◐ | ● | ◐ | ● | ◐ | ◐ | **●** | |
| Search across multiple files | ● | ○ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Regex / whole-word / case options | ◐ | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ◐ | **◐** (case + whole word) | ● |
| Night / dark document mode | ● | ● | ● | ◐ | ● | ● | ◐ | ◐ | ◐ | ● | **●** | |
| App dark theme | ● | ● | ● | ● | ● | ● | ● | ○ | ○ | ◐ | **●** | |
| Rotate view (non-destructive) | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Tabs / multiple docs in one window | ● | ● | ● | ● | ● | ● | ◐ | ● | ● | ● | ○ (one doc per window) | ● |
| Split view / compare side by side | ● | ◐ | ● | ● | ○ | ● | ○ | ◐ | ○ | ○ | ○ | ● |
| Reading mode (chrome hidden) | ● | ● | ● | ● | ◐ | ● | ● | ● | ● | ● | **●** | |
| Presentation / full screen | ● | ● | ● | ● | ◐ | ● | ● | ● | ● | ● | **●** | |
| Read aloud / TTS | ● | ○ | ● | ● | ○ | ● | ● (VoiceOver) | ◐ | ○ | ○ | ○ | ◐ |

### 2.2 Annotation

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Highlight | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Underline | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Strikeout | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Squiggly | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Sticky note (Text annot) | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Freehand / ink | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Eraser for ink | ● | ● | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | **●** | |
| Shapes: rect / ellipse / line / arrow | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Polygon / polyline / cloud | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Free text box (FreeText annot) | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Callout / text with leader | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Stamps (built-in library) | ● | ● | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | **◐** (approved/도장 set) | ● (custom lib) |
| Custom / image stamp | ● | ● | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | ○ | ● |
| Signature: draw / type / image | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ● | **●** | |
| Digital signature (PKI, 공인인증) | ● | ○ | ● | ● | ○ | ◐ | ○ | ● | ○ | ◐ | ○ | ◐ |
| Measurement tools | ● | ○ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ○ |
| Annotation replies / threads | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Author name + timestamp | ● | ● | ● | ● | ◐ | ● | ◐ | ● | ◐ | ◐ | **●** | |
| Annotation summary / export | ● | ◐ | ● | ● | ○ | ◐ | ○ | ● | ○ | ○ | ○ | ● |
| Flatten annotations on export | ● | ● | ● | ● | ◐ | ● | ◐ | ● | ◐ | ◐ | **●** | |
| Copy text from selection | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |

### 2.3 Content editing

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Edit existing text (in-place) | ● | ● | ● | ● | ● | ● | ○ | ● | ○ | ● | **◐** single-line run, same font | ● paragraph reflow |
| Add new text block | ● | ● | ● | ● | ● | ● | ○ | ● | ○ | ● | **●** | |
| Change font / size / colour of edited text | ● | ● | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | **◐** size+colour; family limited to embedded+system | ● |
| Korean font handling when editing | ● | ◐ | ● | ◐ | ◐ | ◐ | — | ● | — | ● | **●** (must be right — see §8.1) | |
| Insert / replace / crop image | ● | ● | ● | ● | ● | ● | ○ | ● | ○ | ● | **●** insert+move+resize+delete | ● crop/replace |
| Edit vector objects | ◐ | ○ | ◐ | ◐ | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ○ |
| Links: create / edit / remove | ● | ● | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | ○ | ● |
| Auto-detect URLs → links | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Watermark (text / image) | ● | ◐ | ● | ● | ● | ● | ○ | ● | ○ | ● | **●** text; image later | ● |
| Header / footer | ● | ○ | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | ○ | ● |
| Page numbers (Bates-lite) | ● | ○ | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | ○ | ● |
| Background colour / image | ● | ○ | ● | ● | ○ | ● | ○ | ● | ○ | ◐ | ○ | ● |
| Bates numbering (legal) | ● | ○ | ● | ● | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ○ |

### 2.4 Page management

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Organize-pages grid mode | ● | ● | ● | ● | ● | ● | ◐ (sidebar only) | ● | ◐ | ● | **●** | |
| Reorder by drag | ● | ● | ● | ● | ● | ● | ● | ● | ◐ | ● | **●** | |
| Delete pages | ● | ● | ● | ● | ● | ● | ● | ● | ◐ | ● | **●** | |
| Rotate pages (destructive) | ● | ● | ● | ● | ● | ● | ● | ● | ◐ | ● | **●** | |
| Insert blank page | ● | ● | ● | ● | ● | ● | ○ | ● | ○ | ● | **●** | |
| Insert pages from file | ● | ● | ● | ● | ● | ● | ● (drag) | ● | ○ | ● | **●** | |
| Extract pages → new doc | ● | ● | ● | ● | ● | ● | ◐ | ● | ○ | ● | **●** | |
| Duplicate page | ● | ● | ● | ● | ◐ | ● | ● | ● | ○ | ● | **●** | |
| Merge documents | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Split by page count / bookmarks | ● | ◐ | ● | ● | ● | ● | ○ | ● | ◐ | ● | **◐** by range/every-N | ● by bookmark |
| Crop pages | ● | ● | ● | ● | ◐ | ● | ● (via inspector) | ● | ○ | ● | ○ | ● |
| Resize / scale pages | ● | ○ | ● | ● | ◐ | ◐ | ○ | ● | ○ | ◐ | ○ | ● |
| Page labels (i, ii, 1, 2…) | ● | ○ | ● | ● | ○ | ○ | ○ | ◐ | ○ | ○ | ○ | ● |
| Reverse page order | ◐ | ◐ | ● | ● | ● | ● | ○ | ● | ○ | ● | **●** | |

### 2.5 Forms

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Fill AcroForm fields | ● | ● | ● | ● | ● | ● | ● | ● | ◐ | ● | **●** | |
| Highlight existing fields | ● | ● | ● | ● | ◐ | ● | ○ | ● | ○ | ◐ | **●** | |
| Checkbox / radio / combo / list | ● | ● | ● | ● | ◐ | ● | ◐ | ● | ○ | ◐ | **●** | |
| Save filled values (incremental) | ● | ● | ● | ● | ● | ● | ● | ● | ◐ | ● | **●** | |
| Flatten form → static | ● | ● | ● | ● | ◐ | ● | ◐ | ● | ○ | ◐ | **●** | |
| Form field *authoring* | ● | ○ | ● | ● | ○ | ◐ | ○ | ● | ○ | ○ | ○ | ● |
| Field validation / calculation / JS | ● | ○ | ● | ● | ○ | ○ | ○ | ◐ | ○ | ○ | ○ | ○ |
| XFA forms | ◐ | ○ | ◐ | ○ | ○ | ○ | ○ | ◐ | ○ | ○ | ○ | ○ |
| "Fill & Sign" non-form typing | ● | ● | ● | ● | ● | ● | ● | ● | ○ | ● | **●** (reuse FreeText) | |
| Export form data (FDF/CSV) | ● | ○ | ● | ● | ○ | ○ | ○ | ● | ○ | ○ | ○ | ● |

### 2.6 OCR

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| OCR present | ● | ◐ (Pro) | ● | ● | ● | ● | ◐ (Live Text) | ● | ● (한컴 OCR) | ◐ (module DL) | **●** | |
| Korean recognition | ● | ◐ | ● | ◐ | ● | ● | ● | ● | ● | ● | **●** | |
| English recognition | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Mixed ko+en in one pass | ● | ◐ | ● | ◐ | ● | ● | ● | ● | ● | ● | **●** | |
| Other languages | ● (~20+) | ◐ | ● (~40) | ● | ● (30+) | ● (30+) | ● | ◐ (ko/en/jp/cn) | ◐ | ◐ | **◐** ko/en/ja/zh | ● pluggable packs |
| Searchable-PDF output (invisible text layer) | ● | ● | ● | ● | ● | ● | ○ | ● | ◐ | ● | **●** | |
| OCR page range selection | ● | ◐ | ● | ● | ◐ | ● | — | ● | ◐ | ◐ | **●** | |
| Auto-deskew / rotate detection | ● | ◐ | ● | ● | ◐ | ◐ | — | ● | ◐ | ◐ | **◐** rotation only | ● deskew |
| Batch OCR many files | ● | ○ | ● | ● | ◐ | ● | ○ | ● | ◐ | ◐ | ○ | ● |
| Editable-text output mode | ● | ○ | ● | ● | ● | ● | ○ | ● | ● | ● | ○ | ● |
| Progress + cancel | ● | ◐ | ● | ● | ◐ | ● | — | ● | ◐ | ◐ | **●** | |
| Offline / no cloud | ● | ◐ | ● | ● | ◐ (cloud for some) | ◐ | ● | ● | ● | ● | **● (hard requirement)** | |

### 2.7 Security

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Open password (user pw) | ● | ● | ● | ● | ● | ● | ● | ● | ◐ | ● | **●** set + remove | |
| Permissions password (owner pw) | ● | ◐ | ● | ● | ◐ | ● | ◐ | ● | ○ | ● | **●** | |
| Open encrypted docs (prompt) | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| AES-256 | ● | ● | ● | ● | ◐ | ● | ● | ● | ◐ | ◐ | **●** | |
| Permission flags (print/copy/edit) | ● | ◐ | ● | ● | ◐ | ● | ◐ | ● | ○ | ◐ | **●** | |
| True redaction (content removal) | ● | ◐ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ◐ | **●** (differentiator) | |
| Redaction search-and-redact | ● | ○ | ● | ● | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ● |
| Sanitise / remove metadata | ● | ○ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ◐ | **●** | |
| Remove hidden data (JS, embedded files) | ● | ○ | ● | ● | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ● |
| Certificate / DRM | ● | ○ | ● | ● | ○ | ○ | ○ | ◐ | ○ | ○ | ○ | ○ |

### 2.8 Convert / export / print / misc

| Feature | AcrP | PDFx | Fox | Nit | PDFg | UPDF | Prev | ezPDF | 한PDF | 알PDF | **v1** | **later** |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Export page → PNG/JPEG | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Export → TIFF | ● | ○ | ● | ● | ◐ | ◐ | ● | ● | ● | ◐ | ○ | ● |
| Export → plain text | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Export → Word/Excel/PPT | ● | ◐ (cloud) | ● | ● | ● | ● | ○ | ● | ● | ● | ○ | ● (needs 3rd-party) |
| Export → hwp/hwpx | ○ | ○ | ○ | ○ | ○ | ○ | ○ | ◐ | ● | ◐ | ○ | ◐ |
| Import Office → PDF | ● | ○ | ● | ● | ● | ● | ◐ (print) | ● | ● | ● | ○ | ○ |
| Image(s) → PDF | ● | ◐ | ● | ● | ● | ● | ● | ● | ● | ● | **●** | |
| Print with page scaling / booklet | ● | ● | ● | ● | ● | ● | ● | ● | ● | ● | **◐** system print + range | ● booklet |
| Compress / optimise | ● | ● | ● | ● | ● | ● | ◐ (Quartz filter) | ● | ● | ● | **◐** image downsample presets | ● full optimiser |
| Compare two documents | ● | ○ | ● | ● | ○ | ◐ | ○ | ◐ | ○ | ○ | ○ | ● |
| Batch processing / actions | ● | ○ | ● | ● | ◐ | ● | ○ | ● | ◐ | ● | ○ | ● |
| Command-line / automation | ◐ | ○ | ◐ | ◐ | ○ | ○ | ● (via CLI tools) | ○ | ○ | ○ | ○ | ◐ |
| AI summarise / chat | ● | ◐ | ● | ● | ● | ● | ○ | ◐ | ◐ | ● | ○ | ◐ |
| Cloud sync | ● | ● | ● | ● | ◐ | ● | ● (iCloud) | ○ | ○ | ◐ | ○ | ○ |

### 2.9 Score summary (rough, v1 target vs field)

| | Viewing | Annotation | Editing | Pages | Forms | OCR | Security | Convert |
|---|:-:|:-:|:-:|:-:|:-:|:-:|:-:|:-:|
| Acrobat Pro | 10 | 10 | 10 | 10 | 10 | 10 | 10 | 10 |
| PDF Expert | 9 | 8 | 6 | 8 | 5 | 4 | 4 | 4 |
| ezPDF Editor 3 | 7 | 8 | 7 | 8 | 9 | 7 | 7 | 7 |
| 알PDF | 6 | 6 | 6 | 7 | 5 | 5 | 5 | 7 |
| Preview.app | 7 | 6 | 1 | 5 | 4 | 1 | 4 | 4 |
| **SeePDF v1** | **8** | **8** | **5** | **8** | **6** | **7** | **7** | **4** |

We deliberately sit *at PDF Expert's level of polish with ezPDF's Korean-market competence*, minus conversion.

---

## 3. What v1 must be, in one sentence

> **SeePDF v1 is Preview.app's speed with ezPDF's Korean competence: open anything instantly, annotate and sign,
> reorganise pages, fill forms, OCR Korean scans offline, and redact — with a native, quiet, Korean-first UI.**

Explicit non-goals for v1: Office conversion, batch pipelines, form authoring, Bates, compare, AI, cloud, tabs.

---

## 4. UI specification — SeePDF v1

### 4.0 Layout decision (the headline)

**Single-window, mode-switcher shell — PDF Expert's model, not Acrobat's ribbon and not UPDF's icon rail.**

Rationale:
- A ribbon (Foxit/Nitro/ezPDF) costs 96–120 px of vertical chrome and reads as "Windows enterprise software". Our
  primary user is on macOS, and the product promise is *lightweight*.
- An icon-only left rail (UPDF) forces tooltips for everything and fights with the thumbnail sidebar for the left edge.
- A **single 44 px toolbar whose right half swaps by mode** gives us Acrobat-level tool count with Preview-level calm,
  and it is the layout Korean users already understand from 알PDF/한/PDF (toolbar on top, thumbnails on the left).

```
┌───────────────────────────────────────────────────────────────────────────────────────────┐
│ ⌃ traffic lights │  ☰   report-2026.pdf ⌄          [읽기][주석][편집][페이지][양식]  │ 🔍  ⋯ │  ← 44px title-bar toolbar
├──────────────┬────────────────────────────────────────────────────────────┬───────────────┤
│              │  ┌──────── contextual tool strip (mode-dependent) ───────┐  │               │
│  SIDEBAR     │  │ ✏️ 🖍 ⌒ S̶  💬  ✎  ▭ ◯ ╱ ↗  T  ⬛  ✍         │  │  PROPERTIES   │  ← 40px, only in Annotate/Edit/Form
│  240px       │  └───────────────────────────────────────────────────────┘  │  260px        │
│              │                                                             │  (collapsible)│
│ ┌──┬──┬──┬──┐│                     ┌───────────────┐                       │               │
│ │▤ │≡ │💬│🔍││                     │               │                       │  Color        │
│ └──┴──┴──┴──┘│                     │   PAGE 1      │                       │  ●●●●●●       │
│              │                     │               │                       │  Opacity ──●─ │
│  [thumb 1]   │                     │               │                       │  Thickness ─● │
│  [thumb 2]   │                     └───────────────┘                       │  Note…        │
│  [thumb 3]   │                     ┌───────────────┐                       │               │
│              │                     │   PAGE 2      │                       │               │
├──────────────┴────────────────────────────────────────────────────────────┴───────────────┤
│  ◀  3 / 148  ▶      │  단일  연속  두 쪽  │      − ────●──── +   118%  ⌄  │  저장됨 ✓      │  ← 28px status bar
└───────────────────────────────────────────────────────────────────────────────────────────┘
```

Tauri config for the title-bar toolbar: `"titleBarStyle": "Overlay"` + `"hiddenTitle": true` on macOS, and a custom
`data-tauri-drag-region` strip on Windows (no native caption; draw our own minimise/maximise/close at the right).
Reserve 78 px on the left for macOS traffic lights (`env(titlebar-area-x)` is not available in a WKWebView, so
hard-code `padding-left: 78px` under `[data-os="macos"]`).

### 4.1 Title-bar toolbar (44 px, always visible)

| Zone | Contents | Notes |
|---|---|---|
| Left (78 px mac / 8 px win) | traffic-light gutter | drag region |
| Left group | `☰` sidebar toggle · `⌫`/`⌦` undo/redo | undo/redo only when a document is open |
| Centre-left | document title + `⌄` (click → path, reveal in Finder/Explorer, rename) | truncates with ellipsis in the middle |
| Centre | **Mode switcher**: 읽기 / 주석 / 편집 / 페이지 / 양식 | segmented control, 5 segments, keyboard `⌘1`–`⌘5` |
| Right | `🔍` search toggle · `⤓` export · `⋯` overflow menu | overflow holds print, security, OCR, doc info, settings |

The mode switcher is the primary navigation. Changing mode changes: the contextual tool strip, the canvas cursor
default, the right panel's content, and what a click on the page does.

### 4.2 Left sidebar (240 px default, 180–420 px drag-resizable, collapsible to 0)

Four tabs as 28 px icon buttons at the top of the sidebar:

| Tab | Icon (lucide) | Content | Empty state |
|---|---|---|---|
| 페이지 (Thumbnails) | `rectangle-vertical` / `layout-grid` | vertical thumbnail list, 1 or 2 columns depending on width, page number under each, current page highlighted with 2 px accent ring, drag to reorder, multi-select with ⌘/⇧ | — |
| 목차 (Outline) | `list-tree` | PDF outline tree, disclosure triangles, current section auto-highlighted on scroll | "이 문서에는 목차가 없습니다" |
| 주석 (Annotations) | `message-square` | flat list grouped by page: icon, author, excerpt/note text, timestamp; click → scroll to + select; right-click → delete; filter chips by type | "주석이 없습니다" |
| 검색 (Search) | `search` | query field, options (대소문자 구분 / 단어 단위), result count, results with ±40 char context and the match bolded, grouped by page | "검색 결과가 없습니다" |

Sidebar state (open/closed, active tab, width) persists per app, not per document, via `tauri-plugin-store`.

### 4.3 Main canvas

- Virtualised scroller: only pages within `viewport ± 1.5 screens` are rendered at full resolution; others show a
  placeholder box of the right aspect ratio with a low-res (≤ 96 px wide) preview if one is cached.
- Page gap 16 px, page shadow `0 1px 3px rgb(0 0 0 / .18)`, canvas background `--bg-canvas` (light `#8a8f98` at 18 %
  tint of the neutral; dark `#141517`).
- Render at `devicePixelRatio × zoom`, capped at 4×. Re-render on zoom settle (120 ms debounce); during the gesture
  scale the existing bitmap (CSS transform) so zoom feels instant.
- Text layer: transparent absolutely-positioned spans over the bitmap for selection/search-highlight, generated from
  PDFium char boxes, only for pages in view.
- Annotation layer: SVG overlay above the text layer; hit-testing in SVG, not in canvas.

### 4.4 Right properties panel (260 px, collapsible, auto-opens)

Opens automatically when a tool with properties is armed or an object is selected; closes when selection is cleared
in 읽기 mode. Never opens in 읽기 mode. Contents by context:

| Context | Panel content |
|---|---|
| Markup tool armed / markup selected | colour swatches (8) + custom, opacity slider, blend note, "메모" textarea, author, created-at, 삭제 button |
| Ink | colour, thickness (1/2/4/8/12 px), opacity, 지우개 크기 |
| Shape | stroke colour, fill colour (+ none), thickness, line style (solid/dashed), arrow heads (line only), opacity |
| Free text / stamp | font family, size, colour, alignment, border, background, opacity |
| Text object (편집 mode) | font family (read-only if not substitutable), size, colour, bold/italic toggles (synthetic), letter spacing |
| Image object | position X/Y, size W/H (link aspect), opacity, 앞으로/뒤로 |
| Page selected (페이지 mode) | page size, rotation, 회전/삭제/추출/복제 buttons |
| Form field focused | field name (read-only), type, value, required flag, 초기화 |
| Redaction pending | fill colour (black default), overlay text, "적용" (destructive, confirms) |

### 4.5 Bottom status bar (28 px)

`◀ [page input] / [total] ▶` · view-mode segmented (단일 / 연속 / 두 쪽) · rotate-view buttons ·
zoom `− [slider] +` + numeric combo (25/50/75/100/125/150/200/400 %, 페이지 맞춤, 너비 맞춤, 실제 크기) ·
right-aligned save state (`저장됨` / `저장되지 않은 변경 사항` / spinner `저장 중…`) and a progress slot used by
OCR/export (indeterminate bar + cancel ×).

### 4.6 Tool modes, cursors, and click behaviour

| Mode | Tool | Cursor | Click / drag on page | Esc returns to |
|---|---|---|---|---|
| 읽기 | 선택(텍스트) | `text` (I-beam) | drag selects text; click clears | — |
| 읽기 | 손(팬) | `grab` / `grabbing` | drag pans | 선택 |
| 읽기 | 스냅샷 | `crosshair` | marquee → copies region as image | 선택 |
| 주석 | 형광펜 | `text` + colour dot badge | drag over text → Highlight annot | 선택 |
| 주석 | 밑줄 | `text` | drag over text → Underline | 선택 |
| 주석 | 취소선 | `text` | drag over text → StrikeOut | 선택 |
| 주석 | 메모 | `copy` (note badge) | click → sticky note at point, popover opens focused | 선택 |
| 주석 | 펜(자유형) | `crosshair` 1 px dot | drag → ink path; ⇧ constrains straight | 선택 |
| 주석 | 지우개 | circle cursor sized to eraser | drag removes ink strokes it touches | 펜 |
| 주석 | 사각형/타원 | `crosshair` | drag; ⇧ = square/circle; ⌥ = from centre | 선택 |
| 주석 | 선/화살표 | `crosshair` | drag; ⇧ snaps to 15° | 선택 |
| 주석 | 텍스트 상자 | `crosshair` | drag box or click for auto-size; enters edit | 선택 |
| 주석 | 도장 | stamp preview ghost follows cursor | click places | 선택 |
| 주석 | 서명 | ghost preview | click places; first use opens 서명 만들기 | 선택 |
| 편집 | 선택 | `default` arrow | click selects object; drag moves; handles resize | — |
| 편집 | 텍스트 추가 | `crosshair` | click → new text box, caret placed | 선택 |
| 편집 | 이미지 추가 | `crosshair` | click or drag → file picker → place | 선택 |
| 편집 | 텍스트 수정 (in-place) | `text` over a text run | double-click a run → inline caret in that run | 선택 |
| 페이지 | — | `default` | grid; click selects, drag reorders, double-click opens that page in 읽기 | — |
| 양식 | 채우기 | `default`, `text` over text fields, `pointer` over buttons | click focuses field | — |
| 보안 | 영역 표시(redact) | `crosshair` | drag marks region; text drag marks runs | 선택 |

Cursor implementation: CSS `cursor` with custom 24×24 PNG/SVG data-URIs at 1× and 2×
(`cursor: url(x.svg) 12 12, crosshair`). Always give a native fallback after the comma.

**Sticky tools:** holding a tool key (e.g. `H` for highlighter) *momentarily* switches while held and reverts on
release, like Figma. Tapping it latches. This is a small detail that makes the app feel professional.

### 4.7 Keyboard shortcuts

`⌘` = Cmd on macOS → Ctrl on Windows unless stated. `⌥` = Option → Alt. `⇧` = Shift.

#### File
| Action | macOS | Windows |
|---|---|---|
| 열기 | ⌘O | Ctrl+O |
| 최근 항목 열기 | ⇧⌘O | Ctrl+Shift+O |
| 닫기 | ⌘W | Ctrl+W |
| 저장 | ⌘S | Ctrl+S |
| 다른 이름으로 저장 | ⇧⌘S | Ctrl+Shift+S |
| 내보내기… | ⌥⌘E | Ctrl+Alt+E |
| 인쇄 | ⌘P | Ctrl+P |
| 문서 정보 | ⌘I | Ctrl+D |
| 설정 | ⌘, | Ctrl+, |
| 종료 | ⌘Q | Alt+F4 |

#### Edit
| Action | macOS | Windows |
|---|---|---|
| 실행 취소 / 다시 실행 | ⌘Z / ⇧⌘Z | Ctrl+Z / Ctrl+Y |
| 잘라내기 / 복사 / 붙여넣기 | ⌘X / ⌘C / ⌘V | Ctrl+X / C / V |
| 전체 선택 | ⌘A | Ctrl+A |
| 삭제 | ⌫ or ⌦ | Delete |
| 복제 | ⌘D | Ctrl+D (only in 페이지 mode) |
| 찾기 | ⌘F | Ctrl+F |
| 다음 찾기 / 이전 찾기 | ⌘G / ⇧⌘G | F3 / Shift+F3 |

#### View
| Action | macOS | Windows |
|---|---|---|
| 사이드바 토글 | ⌃⌘S | Ctrl+F9 |
| 속성 패널 토글 | ⌥⌘P | Ctrl+F10 |
| 축소판 / 목차 / 주석 / 검색 탭 | ⌘⌥1…4 | Ctrl+Alt+1…4 |
| 확대 / 축소 | ⌘+ / ⌘− | Ctrl++ / Ctrl+− |
| 실제 크기 | ⌘0 | Ctrl+0 |
| 페이지에 맞춤 | ⌘9 | Ctrl+9 |
| 너비에 맞춤 | ⌘8 | Ctrl+8 |
| 단일 / 연속 / 두 쪽 | ⌃1 / ⌃2 / ⌃3 | Ctrl+Shift+1/2/3 |
| 왼쪽/오른쪽 회전(보기) | ⌘L / ⌘R | Ctrl+L / Ctrl+R |
| 야간 모드 | ⌃⌘N | Ctrl+Shift+N |
| 읽기 모드 | ⌃⌘R | F8 |
| 전체 화면 | ⌃⌘F | F11 |

#### Navigation
| Action | macOS | Windows |
|---|---|---|
| 다음/이전 페이지 | ↓/↑, PageDown/PageUp, Space/⇧Space | same |
| 첫/마지막 페이지 | ⌘↑ / ⌘↓ (Home/End) | Ctrl+Home / Ctrl+End |
| 페이지로 이동 | ⌥⌘G | Ctrl+G |
| 뒤로 / 앞으로 (link jumps) | ⌘[ / ⌘] | Alt+← / Alt+→ |

#### Modes & tools (single letters, active when canvas has focus and no text field is editing)
| Action | Key |
|---|---|
| 읽기 / 주석 / 편집 / 페이지 / 양식 모드 | ⌘1 / ⌘2 / ⌘3 / ⌘4 / ⌘5 |
| 선택 도구 | V |
| 손 도구 (팬) | Space (hold) |
| 형광펜 / 밑줄 / 취소선 | H / U / K |
| 메모 | N |
| 펜 / 지우개 | P / E |
| 사각형 / 타원 / 선 / 화살표 | R / O / L / A |
| 텍스트 상자 | T |
| 도장 / 서명 | S / G |
| 영역 표시(redact) | ⇧R |
| 도구 해제 (→ 선택) | Esc |

> Conflict note: `Space` is pan-while-held **and** page-down when no tool is armed. Resolution: Space pages down on a
> tap (< 200 ms, no movement) and pans on hold+drag. This matches Preview + Figma expectations. Do not use `H` for hand.

#### Pages mode
| Action | macOS | Windows |
|---|---|---|
| 회전 | ⌘L / ⌘R | Ctrl+L / Ctrl+R |
| 삭제 | ⌫ | Delete |
| 추출 | ⌥⌘X | Ctrl+Alt+X |
| 페이지 삽입 | ⌥⌘I | Ctrl+Alt+I |
| 모두 선택 | ⌘A | Ctrl+A |

### 4.8 Context menus

**On text selection (읽기/주석):** 복사 · 형광펜 · 밑줄 · 취소선 · 메모 추가 · 영역 표시로 표시 · 검색 · 웹에서 검색 …
**On empty page area:** 붙여넣기 · 메모 추가 · 페이지 회전 · 이 페이지 이미지로 내보내기 · 스냅샷 · 페이지로 이동…
**On an annotation:** 편집 · 속성… · 메모 열기 · 맨 앞으로/맨 뒤로 · 복사 · 삭제 · 이 스타일을 기본값으로
**On a thumbnail / page tile:** 이 페이지로 이동 · 왼쪽/오른쪽 회전 · 삭제 · 복제 · 추출… · 뒤에 페이지 삽입… · 이미지로 내보내기
**On an outline item:** 이동 · 이름 변경 (v1: read-only, so omit) · 하위 항목 모두 펼치기/접기
**On a search result:** 이동 · 복사 · 이 결과 형광펜
**On a form field:** 값 지우기 · 모든 필드 지우기 · 필드 강조 표시 전환

Every context menu is rendered in the webview (not a native menu) so it can be themed and localised in one place;
use `oncontextmenu` and suppress the WebView default via `preventDefault` plus Tauri's
`"windows": [{ "additionalBrowserArgs": … }]`/macOS `WKWebView` right-click default being already suppressed by CSS
`-webkit-touch-callout` + our handler.

### 4.9 Annotation properties popover

Two surfaces, deliberately:
1. **Inline popover** — appears ~8 px above the selected annotation's bounding box (flips below when near the top),
   280 px wide, rounded 10 px, `--elevation-2`. Holds the *fast* controls only: 8 colour swatches, opacity slider,
   thickness (if applicable), a 메모 button and a 삭제 button. Dismiss on Esc, outside click, or scroll > 40 px.
2. **Right panel** — the full set, including author/date, blend, default-style action. Opening the panel closes the
   popover.

The sticky-note editor is its own small popover (300 × 180 px) anchored to the note icon: author line, timestamp,
auto-growing textarea, 삭제/완료. Saves on blur.

**"이 스타일을 기본값으로 설정"** stores per-tool defaults in the store; this is the single most-requested behaviour in
every review of every tool in §1 and costs almost nothing.

### 4.10 "Organize pages" grid mode (⌘4)

- Canvas is replaced by a responsive grid of page tiles; sidebar switches to thumbnails-off (it would be redundant) or
  stays and is auto-collapsed. Tile width follows a zoom slider in the status bar (80 / 120 / 160 / 220 px).
- Tile = page image + page number chip + rotation badge + selection ring. Hover reveals a mini toolbar on the tile:
  rotate-left, rotate-right, delete, and a `+` between tiles for "insert here".
- Selection: click, ⇧click for range, ⌘click for toggle, marquee drag on empty space, ⌘A.
- Drag & drop: dragging shows a 2 px vertical insertion caret between tiles; dragging multiple shows a stacked ghost
  with a count badge. Dropping a PDF or image file from the OS inserts at the caret.
- A right-side action list (reusing the properties panel slot): 회전 / 삭제 / 추출 / 복제 / 빈 페이지 삽입 /
  파일에서 삽입 / 순서 뒤집기 / 분할….
- All operations are **undoable** and happen in the in-memory document model, committed on save. The status bar shows
  `변경됨 · 페이지 148 → 143`.

### 4.11 OCR dialog

Modal sheet, 520 px wide. Fields:

1. **범위**: 모든 페이지 / 현재 페이지 / 페이지 범위 `[1-14]` (validated, supports `1,3,5-9`)
2. **언어**: multi-select chips — 한국어, English, 日本語, 中文(简体). Default = 한국어 + English (this is the
   Korean-document reality; every scanned Korean office document has some English in it).
3. **출력**: ⦿ 검색 가능한 PDF (원본 이미지 유지 + 보이지 않는 텍스트 레이어)  ○ 텍스트만 추출 (later)
4. **옵션**: ☑ 페이지 회전 자동 감지 · ☐ 이미 텍스트가 있는 페이지 건너뛰기 (default **on**) · 해상도 `자동 / 200 / 300 / 400 DPI`
5. Footer: estimated time (`예상 시간: 약 40초`), 취소 / 시작.

While running the dialog becomes a progress view: per-page progress `12 / 148`, a thumbnail of the page being
processed, elapsed/remaining, and a 취소 button that is *always* live. On completion: inline success bar with
"텍스트 레이어가 추가되었습니다. 이제 검색할 수 있습니다." + 실행 취소 link.

Hard rules: entirely offline; never blocks the UI; results are applied as one undoable transaction.

### 4.12 Save / export flows

**Save (⌘S)** — writes back to the same file using PDFium's incremental save when only annotations/form values
changed, full rewrite otherwise. If the file is read-only or on a read-only volume, fall through to Save As with an
inline explanation. Never silently discard.

**Save As (⇧⌘S)** — native dialog via `tauri-plugin-dialog`, default name `원본이름.pdf` → suggest `원본이름 (사본).pdf`.

**Export (⌥⌘E)** — a small non-modal sheet with a format list on the left and options on the right:

| Format | Options |
|---|---|
| PDF (평면화) | 주석 평면화 · 양식 평면화 · 페이지 범위 |
| PDF (압축) | 품질 프리셋: 최고/균형/최소 크기 (image downsample 300/150/96 DPI) + 예상 크기 |
| PNG / JPEG | 페이지 범위 · DPI (72–600, default 150) · 투명 배경(PNG) · 품질(JPEG) · 파일당 한 페이지 / 하나로 이어붙이기 |
| 텍스트 (.txt) | 페이지 범위 · 레이아웃 유지 시도 |

Export runs on the engine thread with progress in the status bar and is cancellable. On completion show a toast with
"Finder에서 보기" / "폴더 열기".

**Unsaved-changes guard**: closing a window or quitting with a dirty document shows a native-style sheet:
`"report-2026.pdf"의 변경 사항을 저장하시겠습니까?` — 저장 / 저장 안 함 / 취소. Also mark the window dirty
(`window.setDocumentEdited(true)` equivalent — on macOS Tauri exposes this; on Windows show `•` in the title).

### 4.13 Welcome / recent files screen

Shown when no document is open (also as an empty window). Two columns:

- **Left (220 px)**: product wordmark, version, and three big actions: `파일 열기…`, `최근 항목`, `새 문서` (later:
  scan/blank). A quiet "도움말" and "설정" at the bottom.
- **Right**: recent files as a 3–5 column grid of cards: first-page thumbnail (cached to app data as a 200 px WebP),
  filename, folder, relative date (`오늘`, `어제`, `3일 전`), page count, size. Right-click → 열기 / Finder에서 보기 /
  목록에서 제거 / 즐겨찾기. A search field filters the list. Pinned items first.
- The whole window is a drop target: dropping one or more PDFs opens them (multiple → "여러 파일을 하나로 합치기?"
  prompt with 각각 열기 / 하나로 합치기).
- Empty state: a large dashed drop zone, `PDF 파일을 여기에 놓으세요` + `또는 ⌘O로 열기`.

Recents are stored via `tauri-plugin-store` (path, bookmark data on macOS for sandbox-safe reopening, last page,
last zoom, last view mode — restoring the reading position is a small thing users notice immediately).

### 4.14 Theme

Follow the OS by default (`prefers-color-scheme`) with an explicit override in settings: 시스템 설정 / 밝게 / 어둡게.
Apply by stamping `data-theme` on `<html>`; every colour is a token (§6), no colour literal outside the token file.

**Night mode for the document** is separate from the app theme: it inverts the rendered page with a luminance-preserving
filter (`filter: invert(1) hue-rotate(180deg)` on the page bitmap only, not on annotations' own colours — so we render
annotations in a separate layer that is *not* inverted; this is where a naive CSS filter on the container goes wrong).
Offer 3 levels: 끄기 / 어둡게 / 세피아.

### 4.15 i18n

- Library: **i18next + react-i18next**, or a ~60-line custom hook — with 373 flat keys and no runtime language
  negotiation beyond two locales, a custom loader is genuinely viable and saves ~40 kB. Either way keys are flat
  dotted strings and the catalogue in §5 is the contract.
- Default language = Korean, resolved from `navigator.language`; explicit override in settings (한국어 / English).
- Korean is the *design* language: lay out and size components against Korean strings, then check English.
  Korean is typically 10–30 % narrower than English for button labels but taller in line-height terms.
- Numbers/dates via `Intl` with the active locale. Page counts: `{{count}}쪽` (no plural forms in ko; English needs
  `page/pages`, so use i18next plural suffixes `_one`/`_other`).
- Never concatenate; always interpolate. Keep `{{name}}` placeholders identical in both locales.

---

## 5. i18n string catalogue (v1, complete)

Flat dotted keys. `ko` is the default locale and the design reference. Placeholders in `{{…}}` must match exactly
between locales. Count: **373 keys** across 21 namespaces
(menu 57 · prop 33 · ocr 30 · pages 25 · export 25 · tool 21 · settings 20 · sidebar 18 · security 18 · view 17 ·
common 17 · welcome 14 · sign 13 · form 11 · error 11 · dialog 11 · redact 8 · a11y 8 · status 6 · mode 5 · app 5).
This is the complete v1 set — if a string is not in this table, it should not appear in the v1 UI.

### 5.1 `app.*` — application chrome

| Key | ko | en |
|---|---|---|
| `app.name` | SeePDF | SeePDF |
| `app.untitled` | 제목 없음 | Untitled |
| `app.window.document` | {{name}} — SeePDF | {{name}} — SeePDF |
| `app.edited` | 편집됨 | Edited |
| `app.version` | 버전 {{version}} | Version {{version}} |

### 5.2 `menu.*` — application menu bar (macOS) / hamburger menu (Windows)

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
| `menu.tools.merge` | 파일 합치기… | Merge Files… |
| `menu.window` | 윈도우 | Window |
| `menu.window.minimize` | 최소화 | Minimize |
| `menu.window.zoom` | 확대/축소 | Zoom |
| `menu.help` | 도움말 | Help |
| `menu.help.shortcuts` | 단축키 | Keyboard Shortcuts |
| `menu.help.about` | SeePDF 정보 | About SeePDF |
| `menu.settings` | 설정… | Settings… |
| `menu.quit` | SeePDF 종료 | Quit SeePDF |

### 5.3 `mode.*` — mode switcher

| Key | ko | en |
|---|---|---|
| `mode.read` | 읽기 | Read |
| `mode.annotate` | 주석 | Annotate |
| `mode.edit` | 편집 | Edit |
| `mode.pages` | 페이지 | Pages |
| `mode.form` | 양식 | Fill |

### 5.4 `tool.*` — tool strip

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

### 5.5 `sidebar.*`

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

### 5.6 `view.*` — view controls and status bar

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

### 5.7 `prop.*` — properties panel / annotation popover

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

### 5.8 `pages.*` — organize pages

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

### 5.9 `form.*`

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

### 5.10 `sign.*` — signature

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

### 5.11 `ocr.*`

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

### 5.12 `export.*`

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

### 5.13 `security.*`

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
| `redact.applyWarning` | 적용하면 표시한 내용이 문서에서 영구히 제거됩니다. 실행 취소할 수 없습니다. | Applying permanently removes the marked content. This cannot be undone. |
| `redact.overlayText` | 덮어쓸 문구 | Overlay text |
| `redact.marked` | 표시된 영역 {{count}}개 | {{count}} marked area |
| `redact.marked_other` | 표시된 영역 {{count}}개 | {{count}} marked areas |
| `redact.done` | {{count}}개 영역이 제거되었습니다 | {{count}} areas removed |

### 5.14 `welcome.*`

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

### 5.15 `settings.*`

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
| `settings.authorName` | 주석 작성자 이름 | Annotation author name |
| `settings.renderQuality` | 렌더링 품질 | Rendering quality |
| `settings.cacheSize` | 캐시 크기 | Cache size |
| `settings.clearCache` | 캐시 비우기 | Clear cache |
| `settings.resetDefaults` | 기본값으로 되돌리기 | Reset to defaults |

### 5.16 `dialog.*` and `common.*`

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

### 5.17 `status.*` and `error.*`

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

### 5.18 `a11y.*` — screen-reader-only labels

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

### 5.19 Notes for the implementer

- Store as `src/i18n/ko.json` and `src/i18n/en.json`, flat. Add a dev-time assertion that the two files have the
  identical key set (a 10-line script in `scripts/check-i18n.mjs` run in CI) — drift here is the usual failure mode.
- `_other` suffixed keys exist only because English pluralises; Korean duplicates the singular. Keep them.
- Every user-visible string in the tables above has a `ko` value that is **natural Korean, not translated English**:
  prefer 할 수 있습니다 / 해 주세요 register, avoid 하십시오, avoid 당신.
- Terminology consistency (this is what makes Korean software feel professional):
  주석 (annotation) · 축소판 (thumbnail) · 목차 (outline) · 영역 표시 (redaction) · 평면화 (flatten) ·
  쪽 (page, as a counter) vs 페이지 (page, as a noun/UI object) · 내보내기 (export) · 불러오기 (import).

---

## 6. Design tokens

The goal in one line: **quiet, native, dense enough to be professional, with exactly one accent colour.** Nothing
gradient, nothing glassy, no rounded-pill everything. The document is the hero; the chrome recedes.

### 6.1 Typography

```css
--font-ui:
  "Pretendard Variable", Pretendard,
  -apple-system, BlinkMacSystemFont, "Apple SD Gothic Neo",
  "Segoe UI Variable Text", "Segoe UI", "Malgun Gothic",
  system-ui, sans-serif;
--font-mono: ui-monospace, SFMono-Regular, "SF Mono", "Cascadia Mono", Menlo, "D2Coding", monospace;
```

- **Ship Pretendard Variable subset** (`woff2`, Korean subset ~350 kB for the KS X 1001 range, or use the
  `pretendard-dynamic-subset` per-glyph split which loads ~15 kB per page of text). Reason: `-apple-system` gives
  Apple SD Gothic Neo on macOS and Malgun Gothic on Windows — two very different Korean faces with different metrics,
  so a Korean UI laid out on one looks wrong on the other. Pretendard normalises this and is the de-facto Korean
  product font. Keep the system stack as fallback so the app is usable before the font loads.
- Latin and Hangul in Pretendard share a baseline and cap height, which is exactly the problem Malgun Gothic has.
- Do **not** use a variable font weight axis animation; just 400 / 500 / 600.

| Token | Size / line-height / weight | Used for |
|---|---|---|
| `--text-xs` | 11px / 15px / 500 | badges, page chips, timestamps |
| `--text-sm` | 12px / 17px / 400 | sidebar lists, status bar, secondary labels |
| `--text-base` | 13px / 19px / 400 | default UI text, menus, panel labels |
| `--text-md` | 14px / 21px / 500 | dialog body, tool strip labels |
| `--text-lg` | 17px / 24px / 600 | dialog titles, welcome section headers |
| `--text-xl` | 22px / 30px / 600 | welcome screen title |

macOS renders 13px system text as the norm; Windows expects 14px. Add `[data-os="windows"] { font-size: 14px }` on
`:root` and let everything else be `rem`-relative, rather than maintaining two token sets.

### 6.2 Spacing & sizing (4 px base grid)

| Token | Value |
|---|---|
| `--space-0.5` | 2px |
| `--space-1` | 4px |
| `--space-2` | 8px |
| `--space-3` | 12px |
| `--space-4` | 16px |
| `--space-5` | 20px |
| `--space-6` | 24px |
| `--space-8` | 32px |
| `--space-12` | 48px |

Fixed chrome dimensions: titlebar `44px` · tool strip `40px` · status bar `28px` · sidebar default `240px`
(min 180, max 420) · inspector `260px` · control height `28px` (compact) / `32px` (default) / `36px` (dialog primary)
· icon button `28×28` with a `20px` icon · min hit target `28px` (we are a desktop app; 44 px targets would look
bloated, but never go below 24 px).

### 6.3 Radius, borders, elevation

| Token | Value |
|---|---|
| `--radius-sm` | 4px (chips, swatches) |
| `--radius-md` | 6px (buttons, inputs, tool buttons) |
| `--radius-lg` | 10px (popovers, cards, dialogs) |
| `--radius-xl` | 14px (welcome cards, sheets) |
| `--border-width` | 1px |
| `--elevation-1` | `0 1px 2px rgb(0 0 0 / .06), 0 0 0 1px var(--border-subtle)` |
| `--elevation-2` | `0 6px 16px rgb(0 0 0 / .12), 0 0 0 1px var(--border-subtle)` — popovers |
| `--elevation-3` | `0 16px 48px rgb(0 0 0 / .20), 0 0 0 1px var(--border-subtle)` — dialogs |
| `--page-shadow` | `0 1px 3px rgb(0 0 0 / .18), 0 0 0 1px rgb(0 0 0 / .06)` |

### 6.4 Colour — light

```css
:root {
  --accent:            #2F6FEB;   /* single accent; selection, active mode, focus ring */
  --accent-hover:      #2A62D0;
  --accent-subtle:     #E8F0FE;   /* active nav background */
  --accent-contrast:   #FFFFFF;

  --bg-app:            #F5F5F7;   /* window background behind chrome */
  --bg-chrome:         #FAFAFB;   /* toolbar, status bar */
  --bg-panel:          #FFFFFF;   /* sidebar, inspector, dialogs */
  --bg-canvas:         #9DA2AA;   /* the grey around the pages */
  --bg-hover:          rgb(0 0 0 / .045);
  --bg-active:         rgb(0 0 0 / .075);
  --bg-selected:       #E8F0FE;

  --text-primary:      #1A1C1F;
  --text-secondary:    #5C6169;
  --text-tertiary:     #8A9099;
  --text-inverse:      #FFFFFF;
  --text-disabled:     #B3B8BF;

  --border-subtle:     rgb(0 0 0 / .08);
  --border-default:    rgb(0 0 0 / .14);
  --border-strong:     rgb(0 0 0 / .24);
  --focus-ring:        0 0 0 2px #FFFFFF, 0 0 0 4px rgb(47 111 235 / .55);

  --danger:            #D93F3F;
  --danger-subtle:     #FDECEC;
  --warning:           #C77700;
  --success:           #2E9E5B;
}
```

### 6.5 Colour — dark

```css
:root[data-theme="dark"], :root:not([data-theme="light"]) { /* under prefers-color-scheme: dark */
  --accent:            #4E8BFF;
  --accent-hover:      #6B9EFF;
  --accent-subtle:     #1B2942;
  --accent-contrast:   #0B0D10;

  --bg-app:            #1B1D20;
  --bg-chrome:         #232629;
  --bg-panel:          #1F2225;
  --bg-canvas:         #121417;
  --bg-hover:          rgb(255 255 255 / .06);
  --bg-active:         rgb(255 255 255 / .10);
  --bg-selected:       #1B2942;

  --text-primary:      #E8EAED;
  --text-secondary:    #A2A8B0;
  --text-tertiary:     #737A83;
  --text-inverse:      #14161A;
  --text-disabled:     #565C64;

  --border-subtle:     rgb(255 255 255 / .09);
  --border-default:    rgb(255 255 255 / .16);
  --border-strong:     rgb(255 255 255 / .28);
  --focus-ring:        0 0 0 2px #1F2225, 0 0 0 4px rgb(78 139 255 / .6);

  --danger:            #F1706E;  --danger-subtle:  #37211F;
  --warning:           #E0A030;  --success:        #4FBE7C;
}
```

Contrast check: `--text-secondary` on `--bg-panel` is 5.4:1 light / 6.1:1 dark; `--text-tertiary` is 3.2:1 / 3.6:1 and
is therefore only ever used for non-essential metadata at ≥ 12 px, never for labels.

### 6.6 Annotation colour palette (the 8 swatches)

Chosen to be legible over white paper at 35 % alpha for highlights and at 100 % for ink/shapes, and to be
distinguishable for the common deuteranopia case (no red/green adjacent pair).

| # | Name (ko / en) | Hex | Highlight alpha |
|---|---|---|---|
| 1 | 노랑 / Yellow | `#FFD84D` | .40 |
| 2 | 연두 / Green | `#7BD64A` | .38 |
| 3 | 하늘 / Blue | `#4DB8FF` | .38 |
| 4 | 분홍 / Pink | `#FF7FB0` | .38 |
| 5 | 보라 / Purple | `#B07BFF` | .35 |
| 6 | 주황 / Orange | `#FF9A3D` | .38 |
| 7 | 빨강 / Red | `#F5533D` | .32 |
| 8 | 검정 / Black | `#2B2F33` | — (ink/shape only) |

Store swatches as opaque hex; apply `--highlight-alpha` per type (Highlight uses the alpha column; Underline,
StrikeOut, Square, Circle, Ink, FreeText use 1.0 with a separate opacity slider defaulting to 1.0).

### 6.7 Icons

**lucide-react**, pinned, imported per-icon (`import { Highlighter } from "lucide-react"`) so Vite tree-shakes.
Stroke width `1.75` (lucide's default 2 is slightly heavy at 20 px in a dense toolbar), size 20 px in the toolbar,
16 px in lists and menus, `currentColor` throughout.

Icon map for the tools that lucide covers well: `mouse-pointer-2` (select), `hand`, `crop` (snapshot),
`highlighter`, `underline`, `strikethrough`, `message-square-plus` (note), `pen-line` (pen), `eraser`,
`square`, `circle`, `minus` (line), `move-up-right` (arrow), `type` (text box), `stamp`, `signature`,
`image-plus`, `square-pen` (edit text), `square-dashed` (redact), `panel-left`, `panel-right`,
`layout-grid` (pages), `list-tree` (outline), `message-square` (annotations), `search`, `rotate-ccw`, `rotate-cw`,
`zoom-in`, `zoom-out`, `moon`, `scan-text` (OCR), `lock`, `share` / `download` (export), `printer`, `settings`.

Where lucide is wrong for a PDF concept — stamp (도장) and signature in particular — draw 2 custom 20×20 SVGs on
lucide's grid (24 box, 2 px padding, 1.75 stroke, round caps/joins) so they sit in the same visual family.

### 6.8 Motion

| Token | Value | Use |
|---|---|---|
| `--dur-instant` | 90ms | hover, press |
| `--dur-fast` | 140ms | popovers, panel collapse |
| `--dur-slow` | 220ms | mode transitions, sheet present |
| `--ease-out` | `cubic-bezier(.22,.61,.36,1)` | entering |
| `--ease-inout` | `cubic-bezier(.4,0,.2,1)` | moving |

Never animate the canvas scroll or the page bitmap. Honour `prefers-reduced-motion: reduce` by collapsing all
durations to `0ms` except opacity fades at 90 ms.

### 6.9 The "not templated" rules

1. One accent colour, used only for *state*, never for decoration.
2. No shadow on anything that is not floating above content. Panels use a 1 px border, not a shadow.
3. Borders are alpha-on-background, never a fixed grey — that is what makes a dark theme look wrong.
4. Corner radii step with elevation: flat controls 6, floating surfaces 10, sheets 14. Never mix 8 and 12.
5. Icon-only buttons always have a tooltip after 500 ms, with the shortcut appended in a dimmer weight.
6. Selected page in the thumbnail rail uses a 2 px accent ring plus a slight scale (1.0 → 1.0, no scale — use ring
   and a bolder page number) — never a filled accent block, which fights with the thumbnail image.
7. The toolbar never wraps. Below 900 px window width, tool buttons collapse into a `⋯` overflow, mode labels become
   icon-only, and the document title truncates first.

---

## 7. Performance expectations & targets

Users calibrate against Preview.app (instant) and Acrobat (slow). We must sit at the Preview end. Targets are for a
**base M-series Mac, 16 GB, release build, warm file cache**, and for a mid-range Windows laptop (i5-1240P, 16 GB)
where noted as *win*.

| Scenario | Reference behaviour | **SeePDF target** | Hard fail |
|---|---|---|---|
| Cold app launch → welcome screen visible | Preview ~350 ms; Acrobat 2–4 s | **≤ 700 ms** (win ≤ 1.0 s) | > 1.5 s |
| Open 14-page text PDF (tracemonkey), first page painted | Preview ~120 ms; PDF Expert ~150 ms | **≤ 250 ms** | > 600 ms |
| Open 500-page PDF, first page painted | PDF Expert ~300 ms (lazy); Acrobat 1.5–3 s | **≤ 400 ms** — never parse all pages up front | > 1 s |
| Open 500-page PDF, thumbnail rail usable | PDF Expert: placeholders instantly, fills in progressively | **placeholders ≤ 100 ms, visible thumbs ≤ 500 ms**, rest lazily | blocking the UI at all |
| Scroll at 100 % zoom, continuous | PDF Expert holds 60 fps with brief blur on fast fling | **60 fps sustained**; ≤ 2 consecutive dropped frames; low-res placeholder visible ≤ 120 ms before sharp | visible white gaps |
| Fling scroll 100 pages | — | never blocks input; renders settle ≤ 300 ms after scroll stops | queue backlog that renders stale pages |
| Zoom step (⌘+) | — | **visual response ≤ 16 ms** (CSS transform of existing bitmap), sharp re-render ≤ 250 ms | > 500 ms to sharp |
| Pinch zoom | Preview: fluid | continuous transform at 60 fps, re-render on settle (120 ms debounce) | any per-frame PDFium render |
| Page render, A4 @ 150 DPI | PDFium ~8–25 ms for text pages, 40–120 ms for heavy vector | **p50 ≤ 30 ms, p95 ≤ 120 ms** | > 400 ms without showing a placeholder |
| Text search first match, 500 pages | Acrobat ~1–3 s full pass; PDF Expert streams results | **first match ≤ 150 ms**, full pass ≤ 2.5 s, **results stream in** | blocking UI for the full pass |
| Search re-query (cached text) | — | ≤ 50 ms for the full document | — |
| Text selection drag | — | ≤ 16 ms per update | — |
| Annotation create → visible | — | **≤ 16 ms** (draw in the SVG layer optimistically; persist async) | any round-trip wait |
| Undo | — | ≤ 50 ms | — |
| Save 500-page doc, annotations only | Acrobat 1–3 s | **≤ 300 ms** via incremental save | full rewrite when incremental is possible |
| Save As, full rewrite, 50 MB | — | ≤ 2 s, with progress after 400 ms | no progress indication |
| OCR, 1 page A4 @ 300 DPI, ko+en | Acrobat ~1–2 s/page; ezPDF ~1–3 s/page | **≤ 2 s/page**, parallel over cores, cancellable | UI freeze of any length |
| Organize-pages grid, 500 pages, open | — | grid visible ≤ 200 ms with placeholders | — |
| Page reorder drag | — | 60 fps, commit ≤ 16 ms (model only) | re-rendering pages during drag |
| Idle RAM, 500-page doc open | Preview ~250 MB; Acrobat 700 MB+ | **≤ 400 MB** with a bounded bitmap cache | unbounded growth |
| App bundle size | PDF Expert ~180 MB; PDFgear ~120 MB | **≤ 60 MB** installed (Tauri + libpdfium ~7 MB + OCR data) | > 150 MB |

### 7.1 How these are met (architecture implications)

- **Never** call PDFium off the engine thread; but also never let the engine thread be the bottleneck for input.
  Render requests carry a priority and a generation counter; stale requests are dropped, not rendered.
- Two-tier bitmap cache: a tiny always-resident thumbnail cache (96 px, all pages, ~2 MB for 500 pages) and an
  LRU full-res cache bounded by bytes (default 256 MB), evicting furthest-from-viewport first.
- Extract page text lazily on first search, then keep it (a 500-page text PDF is ~1–3 MB of text — cheap) so
  re-queries are pure JS.
- Progressive search: run the pass on the engine thread page-by-page, emit results per page via a Tauri event,
  render them as they arrive. Users perceive "instant" from the first result, not the last.
- Measure with a fixed harness over `fixtures/` (tracemonkey 14p, TAMReview, and a synthetic 500-page doc) and record
  numbers in `docs/perf/baseline.md`. Regressions in these numbers are bugs.

---

## 8. v1 scope — P0 / P1 / P2

### P0 — without these there is no product (must ship in v1)

1. Open (incl. password-protected), render, continuous/single/two-page, zoom modes, pinch zoom, rotate view
2. Thumbnail sidebar with lazy fill; outline sidebar
3. Text selection + copy; in-document search with streamed results and result list
4. Annotation: highlight / underline / strikeout / sticky note / ink + eraser / rect / ellipse / line / arrow /
   free text; select, move, resize, delete, undo/redo; colour + opacity + thickness; annotation list sidebar
5. Save (incremental), Save As, unsaved-changes guard, recent files with restored reading position
6. Organize pages: grid mode, reorder, delete, rotate, duplicate, insert blank, insert from file, extract, merge
7. Form filling (text, checkbox, radio, combo, list) + save values + field highlight
8. OCR: Korean + English, searchable-PDF output, page range, progress + cancel, fully offline
9. Export: PNG/JPEG, plain text, flattened PDF; print via the system dialog
10. Korean + English i18n (the full §5 catalogue), light/dark theme, welcome screen
11. The performance targets in §7 for open, scroll, zoom, search

### P1 — expected by anyone paying money; ship in v1 if the schedule holds, else v1.1

1. Signature (draw / type / image) with saved signatures
2. Redaction (mark + apply, true content removal) — our differentiator vs PDFgear/알PDF
3. Password set/remove + permission flags + remove metadata
4. Add text and add image objects (not in-place editing of existing text)
5. Night mode for the document (dark / sepia)
6. Stamps (a small built-in set incl. 결재/승인/기밀-style Korean stamps)
7. Compress/export presets with estimated size
8. Split document by range / every-N
9. Per-tool default styles ("이 스타일을 기본값으로")
10. Reading mode + full screen

### P2 — post-v1, in roughly this order

1. **In-place text editing** of existing text runs (same font, single line) → then paragraph reflow
2. Document tabs in one window
3. Links: create/edit, auto-detect URLs
4. Headers/footers, page numbers, watermark (image), page backgrounds
5. Bookmarks/outline editing
6. Batch: OCR many files, batch export, watch-folder
7. Compare two documents
8. Crop and resize pages, page labels
9. Form field authoring
10. Export to Word/hwp (requires a third-party engine — evaluate licensing before promising it)
11. Annotation replies/threads, annotation summary export
12. Multi-file search, split view, TTS

### 8.1 Risks that shape scope

| Risk | Impact | Mitigation |
|---|---|---|
| **Korean text editing with PDFium** — editing a text run needs the font; CJK subset fonts in PDFs usually lack the glyphs you want to type, and PDFium's `FPDFText_SetText` path needs a loaded font that covers the new characters | This is why in-place editing is P2, not P0 | When editing, detect coverage; if the embedded font cannot render the new text, either (a) fall back to a bundled Korean font and embed a subset, or (b) refuse with a clear message rather than emitting tofu. Never silently substitute a font with different metrics. |
| OCR engine choice (no system tesseract, no Homebrew) | Blocks P0 item 8 | Separate spike. Options: vendored tesseract via a Rust crate with bundled `kor.traineddata` (~15 MB) and leptonica statically linked; or an ONNX-based recogniser (PaddleOCR-lite) via `ort`. Bundle size and Korean accuracy decide it. Record in `needed_crates`. |
| Incremental save correctness | Data loss = product death | Always write to a temp file and atomically rename; verify the saved file reopens and has the expected page count before replacing the original. |
| Redaction that does not actually remove content | Legal/PR risk; do not ship a fake | Must delete the underlying text/image objects and re-render, not draw a black box. If PDFium cannot do it for a given object type, rasterise the affected region. Test by extracting text from the output. |
| 500-page performance with a naive React list | Misses §7 targets | Virtualised scroller from day one, not retrofitted. |

### 8.2 Open questions for the product owner

1. Pricing/licensing model — affects whether we need an account/activation UI in v1 (currently assumed: none).
2. Windows chrome: custom-drawn caption buttons (consistent, more work) vs native caption (less work, less clean)?
3. Do we need 공인전자서명 / 전자문서 규격 compliance for the target Korean business users? That is a large,
   separate workstream and would change the security scope substantially.
4. Bundled Korean font licence for text-editing fallback (Pretendard is OFL and safe; embedding it into user PDFs
   is permitted under OFL but the subset must carry the licence).
