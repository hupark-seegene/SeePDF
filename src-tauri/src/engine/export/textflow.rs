//! 텍스트 흐름 내보내기 — lossy DOCX / HWPX / HTML / Markdown (v0.3 pkg8, X6;
//! `IPC_CONTRACT.md` §7.7d).
//!
//! No conversion engine: the page's text layer is regrouped into paragraphs and headings, the
//! embedded images are lifted out in reading order, and the result is written as a plain
//! document. Tables, columns side by side, fonts, colours and exact positions are not kept —
//! the dialog says so ("서식 일부 유실").
//!
//! ## Paragraphs and headings
//!
//! The same rules the paragraph probe (`objects::paragraph`) applies around one point, applied
//! to every line of the page in content order: a line joins the paragraph above when its font
//! size is within 15 %, it sits at most 1.8 × size below the previous baseline, it does not jump
//! up (a new column) and its left edge is within 3 × size of the paragraph's. A trailing
//! hyphen before a lower-case letter is dropped; otherwise lines are joined with a space
//! (Korean breaks lines at spaces). A paragraph's size is the median size of its characters;
//! the body size is the size with the most characters in the whole export, and a short
//! paragraph (≤ 200 characters) at ≥ 1.6 / 1.3 / 1.12 × the body size becomes heading 1 / 2 / 3.
//!
//! ## Images
//!
//! Every image object of the page as it is shown (`objects::extract_images`: soft masks and
//! colour conversion applied), PNG, at its on-page size; placed before the first paragraph
//! whose top is below the image's top.
//!
//! ## Containers
//!
//! * DOCX — `[Content_Types].xml`, `_rels/.rels`, `word/document.xml`, `word/styles.xml`
//!   (Normal, Heading 1–3, 맑은 고딕), `word/_rels/document.xml.rels`, `word/media/*.png`.
//! * HWPX (OWPML, KS X 6101) — `mimetype` (stored, first), `version.xml`,
//!   `META-INF/container.xml`, `Contents/content.hpf`, `Contents/header.xml` (fonts, four
//!   character shapes, one paragraph shape, style 바탕글), `Contents/section0.xml`. Images are
//!   **not** written to HWPX: a picture control needs a full shape object, which is not
//!   attempted in a writer that cannot be checked against 한글 here.
//! * HTML — one self-contained UTF-8 file, images as `data:` URLs.
//! * Markdown — `#` headings, images in `<name>_images/` beside the file.

use crate::engine::objects;
use crate::engine::registry::OpenDoc;
use crate::engine::text::layer;
use crate::ipc::types::PageIndex;
use crate::ipc::{EngineError, ErrorCode};
use pdfium_render::prelude::{PdfPageObjectCommon, PdfPageObjectsCommon};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// The four text-flow formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FlowFormat {
    Docx,
    Hwpx,
    Html,
    Md,
}

/// One block of the document, in reading order.
#[derive(Debug, Clone)]
pub enum Block {
    Text {
        text: String,
        size: f32,
    },
    Image {
        png: Vec<u8>,
        width_pt: f32,
        height_pt: f32,
    },
    /// A page boundary (only the HTML and Markdown writers mark it, as a rule).
    PageBreak,
}

/// Everything collected so far; [`finish`](FlowDoc::write) decides the heading levels.
#[derive(Debug, Default)]
pub struct FlowDoc {
    pub blocks: Vec<Block>,
    pub title: String,
}

/// A heading level (1–3) or 0 for a paragraph.
pub fn heading_level(size: f32, body: f32, text: &str) -> u8 {
    if body <= 0.0 || text.chars().count() > 200 {
        return 0;
    }
    let ratio = size / body;
    if ratio >= 1.6 {
        1
    } else if ratio >= 1.3 {
        2
    } else if ratio >= 1.12 {
        3
    } else {
        0
    }
}

struct TextLine {
    text: String,
    size: f32,
    baseline: f32,
    left: f32,
    top: f32,
}

struct Para {
    text: String,
    sizes: Vec<f32>,
    baseline: f32,
    left: f32,
    top: f32,
}

fn median(mut v: Vec<f32>) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// The lines of a page's text layer, with the pdfium-generated line breaks dropped.
fn lines_of(tl: &layer::TextLayer) -> Vec<TextLine> {
    let mut out = Vec::new();
    for line in &tl.lines {
        let first = line.first_char as usize;
        let end = (first + line.char_count as usize).min(tl.chars.len());
        let mut text = String::new();
        let mut sizes = Vec::new();
        for c in &tl.chars[first..end] {
            let Some(ch) = char::from_u32(c.codepoint) else {
                continue;
            };
            if ch == '\r' || ch == '\n' {
                continue;
            }
            if ch.is_whitespace() || c.is_generated() {
                if !text.ends_with(' ') && !text.is_empty() {
                    text.push(' ');
                }
                continue;
            }
            text.push(ch);
            sizes.push(c.font_size);
        }
        let text = text.trim().to_string();
        if text.is_empty() {
            continue;
        }
        out.push(TextLine {
            text,
            size: median(sizes).max(1.0),
            baseline: line.baseline_y,
            left: line.rect.l,
            top: line.rect.t,
        });
    }
    out
}

fn join_line(para: &mut String, next: &str) {
    let ends_hyphen = para.ends_with('-')
        && para[..para.len() - 1]
            .chars()
            .last()
            .is_some_and(|c| c.is_alphabetic());
    if ends_hyphen && next.chars().next().is_some_and(|c| c.is_lowercase()) {
        para.pop();
        para.push_str(next);
    } else {
        para.push(' ');
        para.push_str(next);
    }
}

fn paragraphs_of(lines: Vec<TextLine>) -> Vec<Para> {
    let mut out: Vec<Para> = Vec::new();
    for line in lines {
        let joins = out.last().is_some_and(|p| {
            let size = median(p.sizes.clone()).max(1.0);
            let same_size = (line.size - size).abs() <= size * 0.15;
            let below = p.baseline - line.baseline;
            let near = below > 0.0 && below <= size * 1.8;
            let aligned = (line.left - p.left).abs() <= size * 3.0;
            same_size && near && aligned
        });
        if joins {
            let p = out.last_mut().expect("checked above");
            join_line(&mut p.text, &line.text);
            p.sizes.push(line.size);
            p.baseline = line.baseline;
        } else {
            out.push(Para {
                text: line.text,
                sizes: vec![line.size],
                baseline: line.baseline,
                left: line.left,
                top: line.top,
            });
        }
    }
    out
}

/// Adds one page: its paragraphs in content order with its images slotted in by position.
pub fn collect_page(
    doc: &mut OpenDoc<'_>,
    page: PageIndex,
    flow: &mut FlowDoc,
) -> Result<(), EngineError> {
    let tl = layer::layer(doc, page)?;
    let paras = paragraphs_of(lines_of(&tl));

    // Images with their on-page box (the object's bounds), in object order.
    let extracted = objects::extract_images(doc, page)?;
    let mut images: Vec<(f32, Block)> = Vec::new();
    {
        let pdf_page = doc.page(page)?;
        let objects = pdf_page.objects();
        for image in extracted {
            let Ok(object) = objects.get(image.object_id as usize) else {
                continue;
            };
            let Ok(bounds) = object.bounds() else {
                continue;
            };
            let (w, h) = (bounds.width().value, bounds.height().value);
            if w < 2.0 || h < 2.0 {
                continue;
            }
            images.push((
                bounds.top().value,
                Block::Image {
                    png: image.png,
                    width_pt: w,
                    height_pt: h,
                },
            ));
        }
    }

    if !flow.blocks.is_empty() {
        flow.blocks.push(Block::PageBreak);
    }
    let mut images = images.into_iter().peekable();
    for p in paras {
        while images.peek().is_some_and(|(top, _)| *top > p.top) {
            flow.blocks.push(images.next().expect("peeked").1);
        }
        flow.blocks.push(Block::Text {
            size: median(p.sizes),
            text: p.text,
        });
    }
    flow.blocks.extend(images.map(|(_, b)| b));
    Ok(())
}

impl FlowDoc {
    /// The size with the most characters: the body text.
    pub fn body_size(&self) -> f32 {
        let mut buckets: Vec<(f32, usize)> = Vec::new();
        for b in &self.blocks {
            if let Block::Text { text, size } = b {
                let n = text.chars().count();
                match buckets.iter_mut().find(|(s, _)| (s - size).abs() <= 0.5) {
                    Some((_, count)) => *count += n,
                    None => buckets.push((*size, n)),
                }
            }
        }
        buckets
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(s, _)| s)
            .unwrap_or(0.0)
    }

    /// Writes the document to `out` and returns every file written (the document first).
    pub fn write(&self, format: FlowFormat, out: &Path) -> Result<Vec<PathBuf>, EngineError> {
        let body = self.body_size();
        let written = match format {
            FlowFormat::Html => {
                crate::engine::pages::write_atomic(out, self.html(body).as_bytes())?;
                vec![out.to_path_buf()]
            }
            FlowFormat::Md => self.markdown(body, out)?,
            FlowFormat::Docx => {
                crate::engine::pages::write_atomic(out, &self.docx(body)?)?;
                vec![out.to_path_buf()]
            }
            FlowFormat::Hwpx => {
                crate::engine::pages::write_atomic(out, &self.hwpx(body)?)?;
                vec![out.to_path_buf()]
            }
        };
        Ok(written)
    }

    fn html(&self, body: f32) -> String {
        let mut s =
            String::from("<!DOCTYPE html>\n<html lang=\"ko\">\n<head>\n<meta charset=\"utf-8\">\n");
        s.push_str(&format!("<title>{}</title>\n", xml_escape(&self.title)));
        s.push_str("<style>body{max-width:46em;margin:2em auto;padding:0 1em;font-family:system-ui,'Apple SD Gothic Neo','Malgun Gothic',sans-serif;line-height:1.6}img{max-width:100%;height:auto}hr{border:0;border-top:1px solid #ccc;margin:2em 0}</style>\n</head>\n<body>\n");
        for b in &self.blocks {
            match b {
                Block::Text { text, size } => match heading_level(*size, body, text) {
                    0 => s.push_str(&format!("<p>{}</p>\n", xml_escape(text))),
                    l => s.push_str(&format!("<h{l}>{}</h{l}>\n", xml_escape(text))),
                },
                Block::Image {
                    png,
                    width_pt,
                    height_pt,
                } => s.push_str(&format!(
                    "<p><img alt=\"\" width=\"{}\" height=\"{}\" src=\"data:image/png;base64,{}\"></p>\n",
                    (width_pt * 96.0 / 72.0).round(),
                    (height_pt * 96.0 / 72.0).round(),
                    base64(png)
                )),
                Block::PageBreak => s.push_str("<hr>\n"),
            }
        }
        s.push_str("</body>\n</html>\n");
        s
    }

    fn markdown(&self, body: f32, out: &Path) -> Result<Vec<PathBuf>, EngineError> {
        let stem = out
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "document".into());
        let dir_name = format!("{stem}_images");
        let dir = out.parent().unwrap_or(Path::new(".")).join(&dir_name);
        let mut written = vec![out.to_path_buf()];
        let mut s = String::new();
        let mut n = 0;
        for b in &self.blocks {
            match b {
                Block::Text { text, size } => {
                    let level = heading_level(*size, body, text);
                    if level > 0 {
                        s.push_str(&"#".repeat(level as usize));
                        s.push(' ');
                    }
                    s.push_str(&md_escape(text));
                    s.push_str("\n\n");
                }
                Block::Image { png, .. } => {
                    n += 1;
                    let name = format!("image-{n}.png");
                    std::fs::create_dir_all(&dir).map_err(EngineError::from)?;
                    let path = dir.join(&name);
                    crate::engine::pages::write_atomic(&path, png)?;
                    written.push(path);
                    s.push_str(&format!("![]({}/{})\n\n", url_path(&dir_name), name));
                }
                Block::PageBreak => s.push_str("---\n\n"),
            }
        }
        crate::engine::pages::write_atomic(out, s.as_bytes())?;
        Ok(written)
    }

    fn docx(&self, body: f32) -> Result<Vec<u8>, EngineError> {
        let mut doc = String::from(concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            "\n",
            r#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>"#
        ));
        let mut rels = String::from(concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
            "\n",
            r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#
        ));
        let mut media: Vec<(String, &[u8])> = Vec::new();
        for b in &self.blocks {
            match b {
                Block::Text { text, size } => {
                    let style = match heading_level(*size, body, text) {
                        0 => String::new(),
                        l => format!(r#"<w:pPr><w:pStyle w:val="Heading{l}"/></w:pPr>"#),
                    };
                    doc.push_str(&format!(
                        r#"<w:p>{style}<w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
                        xml_escape(text)
                    ));
                }
                Block::Image {
                    png,
                    width_pt,
                    height_pt,
                } => {
                    let n = media.len() + 1;
                    let name = format!("image{n}.png");
                    // At most the text width of an A4 page with 1-inch margins (6.27 in).
                    let scale = (451.0 / width_pt).min(1.0);
                    let cx = (width_pt * scale * 12700.0).round() as u64;
                    let cy = (height_pt * scale * 12700.0).round() as u64;
                    rels.push_str(&format!(
                        r#"<Relationship Id="rIdImg{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/{name}"/>"#
                    ));
                    doc.push_str(&format!(
                        concat!(
                            r#"<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">"#,
                            r#"<wp:extent cx="{cx}" cy="{cy}"/><wp:docPr id="{n}" name="Picture {n}"/>"#,
                            r#"<a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">"#,
                            r#"<pic:pic><pic:nvPicPr><pic:cNvPr id="{n}" name="{name}"/><pic:cNvPicPr/></pic:nvPicPr>"#,
                            r#"<pic:blipFill><a:blip r:embed="rIdImg{n}"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>"#,
                            r#"<pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="{cx}" cy="{cy}"/></a:xfrm>"#,
                            r#"<a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic>"#,
                            r#"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
                        ),
                        cx = cx,
                        cy = cy,
                        n = n,
                        name = name
                    ));
                    media.push((name, png));
                }
                Block::PageBreak => doc.push_str(r#"<w:p><w:r><w:br w:type="page"/></w:r></w:p>"#),
            }
        }
        doc.push_str(r#"<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440" w:header="708" w:footer="708" w:gutter="0"/></w:sectPr></w:body></w:document>"#);
        rels.push_str("</Relationships>");

        let mut zip = Zip::new();
        zip.add("[Content_Types].xml", DOCX_CONTENT_TYPES.as_bytes(), true)?;
        zip.add("_rels/.rels", DOCX_RELS.as_bytes(), true)?;
        zip.add("word/document.xml", doc.as_bytes(), true)?;
        zip.add("word/styles.xml", DOCX_STYLES.as_bytes(), true)?;
        zip.add("word/_rels/document.xml.rels", rels.as_bytes(), true)?;
        for (name, png) in media {
            zip.add(&format!("word/media/{name}"), png, false)?;
        }
        zip.finish()
    }

    fn hwpx(&self, body: f32) -> Result<Vec<u8>, EngineError> {
        let mut sec = String::from(concat!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
            r#"<hs:sec xmlns:hs="http://www.hancom.co.kr/hwpml/2011/section" xmlns:hp="http://www.hancom.co.kr/hwpml/2011/paragraph" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core">"#
        ));
        let mut texts: Vec<(u8, &str)> = self
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Text { text, size } => {
                    Some((heading_level(*size, body, text), text.as_str()))
                }
                _ => None,
            })
            .collect();
        if texts.is_empty() {
            texts.push((0, ""));
        }
        for (id, (level, text)) in texts.into_iter().enumerate() {
            let char_pr = level; // 0 = body, 1..3 = headings (header.xml)
                                 // The first paragraph carries the section's page setup.
            let sec_pr = if id == 0 { HWPX_SEC_PR } else { "" };
            sec.push_str(&format!(
                r#"<hp:p id="{id}" paraPrIDRef="0" styleIDRef="0" pageBreak="0" columnBreak="0" merged="0"><hp:run charPrIDRef="{char_pr}">{sec_pr}<hp:t>{}</hp:t></hp:run></hp:p>"#,
                xml_escape(text)
            ));
        }
        sec.push_str("</hs:sec>");
        let hpf = format!(
            concat!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
                r#"<opf:package xmlns:opf="http://www.idpf.org/2007/opf/" xmlns:hpf="http://www.hancom.co.kr/schema/2011/hpf" version="" unique-identifier="" id="">"#,
                r#"<opf:metadata><opf:title>{}</opf:title><opf:language>ko</opf:language></opf:metadata>"#,
                r#"<opf:manifest><opf:item id="header" href="Contents/header.xml" media-type="application/xml"/>"#,
                r#"<opf:item id="section0" href="Contents/section0.xml" media-type="application/xml"/>"#,
                r#"<opf:item id="settings" href="settings.xml" media-type="application/xml"/></opf:manifest>"#,
                r#"<opf:spine><opf:itemref idref="header" linear="yes"/><opf:itemref idref="section0" linear="yes"/></opf:spine></opf:package>"#
            ),
            xml_escape(&self.title)
        );
        let mut zip = Zip::new();
        // OCF: the mime type first and uncompressed, so a reader can sniff it at offset 38.
        zip.add("mimetype", b"application/hwp+zip", false)?;
        zip.add("version.xml", HWPX_VERSION.as_bytes(), true)?;
        zip.add("Contents/header.xml", HWPX_HEADER.as_bytes(), true)?;
        zip.add("Contents/section0.xml", sec.as_bytes(), true)?;
        zip.add("Contents/content.hpf", hpf.as_bytes(), true)?;
        zip.add("settings.xml", HWPX_SETTINGS.as_bytes(), true)?;
        zip.add("META-INF/container.xml", HWPX_CONTAINER.as_bytes(), true)?;
        zip.add("META-INF/manifest.xml", HWPX_MANIFEST.as_bytes(), true)?;
        zip.finish()
    }
}

struct Zip(zip::ZipWriter<std::io::Cursor<Vec<u8>>>);

impl Zip {
    fn new() -> Self {
        Self(zip::ZipWriter::new(std::io::Cursor::new(Vec::new())))
    }

    fn add(&mut self, name: &str, data: &[u8], deflate: bool) -> Result<(), EngineError> {
        let method = if deflate {
            zip::CompressionMethod::Deflated
        } else {
            zip::CompressionMethod::Stored
        };
        let options = zip::write::SimpleFileOptions::default().compression_method(method);
        self.0.start_file(name, options).map_err(zip_error)?;
        self.0.write_all(data).map_err(EngineError::from)
    }

    fn finish(self) -> Result<Vec<u8>, EngineError> {
        Ok(self.0.finish().map_err(zip_error)?.into_inner())
    }
}

fn zip_error(e: zip::result::ZipError) -> EngineError {
    EngineError::new(ErrorCode::Io, format!("zip: {e}"))
}

pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 forbids most C0 controls.
            c if (c as u32) < 0x20 && !matches!(c, '\t' | '\n' | '\r') => {}
            c => out.push(c),
        }
    }
    out
}

/// Markdown: characters that would start markup at the head of a line, and inline `*_[]<>\``.
fn md_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, c) in s.chars().enumerate() {
        let lead = i == 0 && matches!(c, '#' | '>' | '-' | '+' | '=' | '|');
        if lead || matches!(c, '\\' | '*' | '_' | '[' | ']' | '<' | '>' | '`') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A relative link path: spaces and the few characters Markdown link syntax trips on escaped.
fn url_path(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            ' ' => "%20".to_string(),
            '(' => "%28".to_string(),
            ')' => "%29".to_string(),
            c => c.to_string(),
        })
        .collect()
}

/// Standard base64 with padding (the HTML `data:` URLs).
pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            chunk.get(1).copied().unwrap_or(0),
            chunk.get(2).copied().unwrap_or(0),
        ];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

const DOCX_CONTENT_TYPES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">"#,
    r#"<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>"#,
    r#"<Default Extension="xml" ContentType="application/xml"/>"#,
    r#"<Default Extension="png" ContentType="image/png"/>"#,
    r#"<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>"#,
    r#"<Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>"#,
    r#"</Types>"#
);

const DOCX_RELS: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
    r#"<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>"#,
    r#"</Relationships>"#
);

const DOCX_STYLES: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>"#,
    "\n",
    r#"<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">"#,
    r#"<w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Malgun Gothic" w:eastAsia="Malgun Gothic" w:hAnsi="Malgun Gothic" w:cs="Malgun Gothic"/><w:sz w:val="21"/><w:szCs w:val="21"/><w:lang w:val="ko-KR" w:eastAsia="ko-KR"/></w:rPr></w:rPrDefault>"#,
    r#"<w:pPrDefault><w:pPr><w:spacing w:after="160" w:line="300" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>"#,
    r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>"#,
    r#"<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="360" w:after="120"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:sz w:val="36"/><w:szCs w:val="36"/></w:rPr></w:style>"#,
    r#"<w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="240" w:after="100"/><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:b/><w:sz w:val="30"/><w:szCs w:val="30"/></w:rPr></w:style>"#,
    r#"<w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="200" w:after="80"/><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/><w:sz w:val="26"/><w:szCs w:val="26"/></w:rPr></w:style>"#,
    r#"</w:styles>"#
);

const HWPX_VERSION: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
    r#"<hv:HCFVersion xmlns:hv="http://www.hancom.co.kr/hwpml/2011/version" tagetApplication="WORDPROCESSOR" major="5" minor="1" micro="0" buildNumber="1" os="1" xmlVersion="1.4" application="SeePDF" appVersion="0.3"/>"#
);

const HWPX_CONTAINER: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
    r#"<ocf:container xmlns:ocf="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:hpf="http://www.hancom.co.kr/schema/2011/hpf">"#,
    r#"<ocf:rootfiles><ocf:rootfile full-path="Contents/content.hpf" media-type="application/hwpml-package+xml"/></ocf:rootfiles></ocf:container>"#
);

const HWPX_MANIFEST: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
    r#"<odf:manifest xmlns:odf="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0"/>"#
);

const HWPX_SETTINGS: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
    r#"<ha:HWPApplicationSetting xmlns:ha="http://www.hancom.co.kr/hwpml/2011/app" xmlns:config="urn:oasis:names:tc:opendocument:xmlns:config:1.0">"#,
    r#"<ha:CaretPosition listIDRef="0" paraIDRef="0" pos="0"/></ha:HWPApplicationSetting>"#
);

/// The first paragraph's section properties: A4 portrait, 30 mm / 20 mm margins.
const HWPX_SEC_PR: &str = concat!(
    r#"<hp:secPr id="" textDirection="HORIZONTAL" spaceColumns="1134" tabStop="8000" tabStopVal="4000" tabStopUnit="HWPUNIT" outlineShapeIDRef="0" memoShapeIDRef="0" textVerticalWidthHead="0" masterPageCnt="0">"#,
    r#"<hp:grid lineGrid="0" charGrid="0" wonggojiFormat="0"/><hp:startNum pageStartsOn="BOTH" page="0" pic="0" tbl="0" equation="0"/>"#,
    r#"<hp:visibility hideFirstHeader="0" hideFirstFooter="0" hideFirstMasterPage="0" border="SHOW_ALL" fill="SHOW_ALL" hideFirstPageNum="0" hideFirstEmptyLine="0" showLineNumber="0"/>"#,
    r#"<hp:pagePr landscape="WIDELY" width="59528" height="84186" gutterType="LEFT_ONLY"><hp:margin header="4252" footer="4252" gutter="0" left="8504" right="8504" top="5668" bottom="4252"/></hp:pagePr>"#,
    r#"</hp:secPr>"#
);

/// Fonts (함초롬바탕 for every script), four character shapes (본문 10 pt, 제목 18 / 15 / 13 pt
/// bold), one paragraph shape (양쪽 정렬, 줄 간격 160 %) and the style 바탕글.
const HWPX_HEADER: &str = concat!(
    r#"<?xml version="1.0" encoding="UTF-8" standalone="yes" ?>"#,
    r#"<hh:head xmlns:hh="http://www.hancom.co.kr/hwpml/2011/head" xmlns:hc="http://www.hancom.co.kr/hwpml/2011/core" version="1.4" secCnt="1">"#,
    r#"<hh:beginNum page="1" footnote="1" endnote="1" pic="1" tbl="1" equation="1"/>"#,
    r#"<hh:refList>"#,
    r#"<hh:fontfaces itemCnt="7">"#,
    r#"<hh:fontface lang="HANGUL" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"<hh:fontface lang="LATIN" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"<hh:fontface lang="HANJA" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"<hh:fontface lang="JAPANESE" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"<hh:fontface lang="OTHER" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"<hh:fontface lang="SYMBOL" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"<hh:fontface lang="USER" fontCnt="1"><hh:font id="0" face="함초롬바탕" type="TTF" isEmbedded="0"/></hh:fontface>"#,
    r#"</hh:fontfaces>"#,
    r#"<hh:borderFills itemCnt="1"><hh:borderFill id="1" threeD="0" shadow="0" centerLine="NONE" breakCellSeparateLine="0">"#,
    r#"<hh:slash type="NONE" Crooked="0" isCounter="0"/><hh:backSlash type="NONE" Crooked="0" isCounter="0"/>"#,
    r##"<hh:leftBorder type="NONE" width="0.1 mm" color="#000000"/><hh:rightBorder type="NONE" width="0.1 mm" color="#000000"/>"##,
    r##"<hh:topBorder type="NONE" width="0.1 mm" color="#000000"/><hh:bottomBorder type="NONE" width="0.1 mm" color="#000000"/>"##,
    r##"<hh:diagonal type="SOLID" width="0.1 mm" color="#000000"/></hh:borderFill></hh:borderFills>"##,
    r#"<hh:charProperties itemCnt="4">"#,
    r##"<hh:charPr id="0" height="1000" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/></hh:charPr>"##,
    r##"<hh:charPr id="1" height="1800" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr>"##,
    r##"<hh:charPr id="2" height="1500" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr>"##,
    r##"<hh:charPr id="3" height="1300" textColor="#000000" shadeColor="none" useFontSpace="0" useKerning="0" symMark="NONE" borderFillIDRef="1"><hh:fontRef hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:ratio hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:spacing hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:relSz hangul="100" latin="100" hanja="100" japanese="100" other="100" symbol="100" user="100"/><hh:offset hangul="0" latin="0" hanja="0" japanese="0" other="0" symbol="0" user="0"/><hh:bold/></hh:charPr>"##,
    r#"</hh:charProperties>"#,
    r#"<hh:tabProperties itemCnt="1"><hh:tabPr id="0" autoTabLeft="0" autoTabRight="0"/></hh:tabProperties>"#,
    r#"<hh:paraProperties itemCnt="1"><hh:paraPr id="0" tabPrIDRef="0" condense="0" fontLineHeight="0" snapToGrid="1" suppressLineNumbers="0" checked="0">"#,
    r#"<hh:align horizontal="JUSTIFY" vertical="BASELINE"/><hh:heading type="NONE" idRef="0" level="0"/>"#,
    r#"<hh:breakSetting breakLatinWord="KEEP_WORD" breakNonLatinWord="KEEP_WORD" widowOrphan="0" keepWithNext="0" keepLines="0" pageBreakBefore="0" lineWrap="BREAK"/>"#,
    r#"<hh:autoSpacing eAsianEng="0" eAsianNum="0"/>"#,
    r#"<hh:margin><hc:intent value="0" unit="HWPUNIT"/><hc:left value="0" unit="HWPUNIT"/><hc:right value="0" unit="HWPUNIT"/><hc:prev value="0" unit="HWPUNIT"/><hc:next value="800" unit="HWPUNIT"/></hh:margin>"#,
    r#"<hh:lineSpacing type="PERCENT" value="160" unit="HWPUNIT"/>"#,
    r#"<hh:border borderFillIDRef="1" offsetLeft="0" offsetRight="0" offsetTop="0" offsetBottom="0" connect="0" ignoreMargin="0"/>"#,
    r#"</hh:paraPr></hh:paraProperties>"#,
    r#"<hh:styles itemCnt="1"><hh:style id="0" type="PARA" name="바탕글" engName="Normal" paraPrIDRef="0" charPrIDRef="0" nextStyleIDRef="0" langID="1042" lockForm="0"/></hh:styles>"#,
    r#"</hh:refList>"#,
    r#"<hh:compatibleDocument targetProgram="HWP201X"><hh:layoutCompatibility/></hh:compatibleDocument>"#,
    r#"<hh:docOption><hh:linkinfo path="" pageInherit="0" footnoteInherit="0"/></hh:docOption>"#,
    r#"</hh:head>"#
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_rfc4648() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn heading_levels() {
        assert_eq!(heading_level(24.0, 11.0, "제목"), 1);
        assert_eq!(heading_level(15.0, 11.0, "소제목"), 2);
        assert_eq!(heading_level(12.5, 11.0, "작은 제목"), 3);
        assert_eq!(heading_level(11.0, 11.0, "본문"), 0);
        assert_eq!(
            heading_level(24.0, 11.0, &"긴".repeat(300)),
            0,
            "a long block is text"
        );
    }

    #[test]
    fn escapes() {
        assert_eq!(xml_escape("a<b & \"c\"\u{1}"), "a&lt;b &amp; &quot;c&quot;");
        assert_eq!(
            md_escape("# not a heading *x*"),
            "\\# not a heading \\*x\\*"
        );
    }

    #[test]
    fn lines_join_into_paragraphs() {
        let line = |text: &str, size: f32, baseline: f32, left: f32| TextLine {
            text: text.into(),
            size,
            baseline,
            left,
            top: baseline + size,
        };
        let paras = paragraphs_of(vec![
            line("큰 제목", 24.0, 780.0, 72.0),
            line("첫 문단의 첫 줄이고", 11.0, 740.0, 72.0),
            line("둘째 줄입니다. hyphen-", 11.0, 726.0, 72.0),
            line("ated word", 11.0, 712.0, 72.0),
            line("새 문단", 11.0, 680.0, 72.0),
        ]);
        let texts: Vec<&str> = paras.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(
            texts,
            vec![
                "큰 제목",
                "첫 문단의 첫 줄이고 둘째 줄입니다. hyphenated word",
                "새 문단"
            ]
        );
    }
}
