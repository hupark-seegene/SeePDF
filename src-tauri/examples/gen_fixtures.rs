//! Deterministic test fixtures for `fixtures/gen/`.
//!
//! ```sh
//! cd src-tauri && cargo run --release --example gen_fixtures
//! ```
//!
//! | file | what it is | how |
//! |---|---|---|
//! | `gen/500p.pdf` | 500 letter pages, each stamped with its number | hand-written PDF |
//! | `gen/outline-labels.pdf` | 6 pages, a 7-node / 3-level outline, `/PageLabels` in three styles (`i, ii, 1, 2, App-A, App-B`) | hand-written PDF |
//! | `gen/encrypted-rc4-40.pdf` | standard security handler V=1 R=2 RC4-40, user `user`, owner `owner`, `/P -21` | hand-written PDF + MD5/RC4 below |
//! | `gen/korean-300dpi.pdf` | a sparse Korean page sized for 300 DPI OCR | pdfium + `resources/fonts/SeePDF-Hangul.ttf` |
//!
//! The first three are **byte-deterministic**: no timestamps, no pdfium, no randomness, so
//! regenerating them produces identical files. The Korean page needs a Hangul font to embed
//! and is therefore skipped (with a printed note) when none is found — PDFium cannot write
//! outlines, page labels or encryption at all (pages spike §5, §6, §8), which is why the
//! other three are written by hand. The Korean page waits for the bundled Hangul subset
//! because PDFium embeds whole font files: Arial Unicode makes it a 15.3 MB fixture.

use std::path::{Path, PathBuf};

fn gen_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("src-tauri has a parent")
        .join("fixtures")
        .join("gen")
}

fn main() {
    let dir = gen_dir();
    std::fs::create_dir_all(&dir).expect("create fixtures/gen");

    write(&dir.join("500p.pdf"), &five_hundred_pages());
    write(&dir.join("outline-labels.pdf"), &outline_labels());
    write(&dir.join("encrypted-rc4-40.pdf"), &encrypted_rc4_40());

    match korean_page() {
        Ok(bytes) => write(&dir.join("korean-300dpi.pdf"), &bytes),
        Err(why) => println!("skipped korean-300dpi.pdf: {why}"),
    }
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
    println!("wrote {} ({} bytes)", path.display(), bytes.len());
}

// ---------------------------------------------------------------------------------------
// A very small PDF writer
// ---------------------------------------------------------------------------------------

/// Objects are pushed in order and referenced by their 1-based number.
#[derive(Default)]
struct Pdf {
    objects: Vec<Vec<u8>>,
}

impl Pdf {
    /// Reserves the next object number without writing it yet.
    fn reserve(&mut self) -> usize {
        self.objects.push(Vec::new());
        self.objects.len()
    }

    fn set(&mut self, number: usize, body: impl AsRef<[u8]>) {
        self.objects[number - 1] = body.as_ref().to_vec();
    }

    fn add(&mut self, body: impl AsRef<[u8]>) -> usize {
        let n = self.reserve();
        self.set(n, body);
        n
    }

    fn stream(&mut self, data: &[u8]) -> usize {
        let mut body = format!("<< /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.add(body)
    }

    fn finish(self, trailer_extra: &str) -> Vec<u8> {
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::with_capacity(self.objects.len());
        for (i, body) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref_at = out.len();
        let size = self.objects.len() + 1;
        out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for offset in offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size {size} {trailer_extra} >>\nstartxref\n{xref_at}\n%%EOF\n")
                .as_bytes(),
        );
        out
    }
}

/// Escapes a PDF literal string; the fixtures only use ASCII.
fn lit(s: &str) -> String {
    let escaped: String = s
        .chars()
        .map(|c| match c {
            '(' => "\\(".to_string(),
            ')' => "\\)".to_string(),
            '\\' => "\\\\".to_string(),
            c => c.to_string(),
        })
        .collect();
    format!("({escaped})")
}

// ---------------------------------------------------------------------------------------
// gen/500p.pdf
// ---------------------------------------------------------------------------------------

fn five_hundred_pages() -> Vec<u8> {
    const COUNT: usize = 500;
    let mut pdf = Pdf::default();
    let catalog = pdf.reserve();
    let pages = pdf.reserve();
    let font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");

    let mut kids = Vec::with_capacity(COUNT);
    for i in 0..COUNT {
        let content = format!(
            "BT /F1 36 Tf 72 640 Td {} Tj ET\n\
             BT /F1 11 Tf 72 600 Td {} Tj ET\n\
             1 0 0 RG 4 w 72 72 m 540 72 l S\n",
            lit(&format!("Page {}", i + 1)),
            lit("fixtures/gen/500p.pdf - generated by src-tauri/examples/gen_fixtures.rs")
        );
        let stream = pdf.stream(content.as_bytes());
        let page = pdf.add(format!(
            "<< /Type /Page /Parent {pages} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {stream} 0 R >>"
        ));
        kids.push(format!("{page} 0 R"));
    }
    pdf.set(
        pages,
        format!(
            "<< /Type /Pages /Count {COUNT} /Kids [{}] >>",
            kids.join(" ")
        ),
    );
    pdf.set(
        catalog,
        format!("<< /Type /Catalog /Pages {pages} 0 R >>"),
    );
    pdf.finish(&format!("/Root {catalog} 0 R"))
}

// ---------------------------------------------------------------------------------------
// gen/outline-labels.pdf
// ---------------------------------------------------------------------------------------

/// 6 pages; labels `i, ii, 1, 2, App-A, App-B` (`/S /r`, `/S /D /St 1`, `/S /A /P (App-)`);
/// outline: Chapter A (A.1, A.2), Chapter B (B.1 (B.1.a)), Chapter C — 7 nodes, 3 levels.
fn outline_labels() -> Vec<u8> {
    const COUNT: usize = 6;
    let mut pdf = Pdf::default();
    let catalog = pdf.reserve();
    let pages_obj = pdf.reserve();
    let font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");

    let titles = [
        "Front matter i",
        "Front matter ii",
        "Chapter A - page 1",
        "Chapter B - page 2",
        "Appendix A",
        "Appendix B",
    ];
    let mut page_objs = Vec::with_capacity(COUNT);
    for title in titles {
        let content = format!("BT /F1 24 Tf 72 680 Td {} Tj ET\n", lit(title));
        let stream = pdf.stream(content.as_bytes());
        page_objs.push(pdf.add(format!(
            "<< /Type /Page /Parent {pages_obj} 0 R /MediaBox [0 0 612 792] \
             /Resources << /Font << /F1 {font} 0 R >> >> /Contents {stream} 0 R >>"
        )));
    }
    pdf.set(
        pages_obj,
        format!(
            "<< /Type /Pages /Count {COUNT} /Kids [{}] >>",
            page_objs
                .iter()
                .map(|n| format!("{n} 0 R"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );

    // Outline tree.
    let outlines = pdf.reserve();
    let (ch_a, a1, a2) = (pdf.reserve(), pdf.reserve(), pdf.reserve());
    let (ch_b, b1, b1a) = (pdf.reserve(), pdf.reserve(), pdf.reserve());
    let ch_c = pdf.reserve();
    let dest = |page: usize| format!("[{} 0 R /Fit]", page_objs[page]);

    pdf.set(
        outlines,
        format!("<< /Type /Outlines /First {ch_a} 0 R /Last {ch_c} 0 R /Count 3 >>"),
    );
    pdf.set(
        ch_a,
        format!(
            "<< /Title {} /Parent {outlines} 0 R /Next {ch_b} 0 R /First {a1} 0 R /Last {a2} 0 R \
             /Count 2 /Dest {} >>",
            lit("Chapter A"),
            dest(0)
        ),
    );
    pdf.set(
        a1,
        format!(
            "<< /Title {} /Parent {ch_a} 0 R /Next {a2} 0 R /Dest {} >>",
            lit("A.1 Introduction"),
            dest(1)
        ),
    );
    pdf.set(
        a2,
        format!(
            "<< /Title {} /Parent {ch_a} 0 R /Prev {a1} 0 R /Dest {} >>",
            lit("A.2 Method"),
            dest(2)
        ),
    );
    pdf.set(
        ch_b,
        format!(
            "<< /Title {} /Parent {outlines} 0 R /Prev {ch_a} 0 R /Next {ch_c} 0 R \
             /First {b1} 0 R /Last {b1} 0 R /Count 2 /Dest {} >>",
            lit("Chapter B"),
            dest(3)
        ),
    );
    pdf.set(
        b1,
        format!(
            "<< /Title {} /Parent {ch_b} 0 R /First {b1a} 0 R /Last {b1a} 0 R /Count 1 /Dest {} >>",
            lit("B.1 Results"),
            dest(4)
        ),
    );
    pdf.set(
        b1a,
        format!(
            "<< /Title {} /Parent {b1} 0 R /Dest {} >>",
            lit("B.1.a Detail"),
            dest(5)
        ),
    );
    pdf.set(
        ch_c,
        format!(
            "<< /Title {} /Parent {outlines} 0 R /Prev {ch_b} 0 R /Dest {} >>",
            lit("Chapter C"),
            dest(5)
        ),
    );

    pdf.set(
        catalog,
        format!(
            "<< /Type /Catalog /Pages {pages_obj} 0 R /Outlines {outlines} 0 R \
             /PageLabels << /Nums [0 << /S /r >> 2 << /S /D /St 1 >> 4 << /S /A /P (App-) >>] >> >>"
        ),
    );
    pdf.finish(&format!("/Root {catalog} 0 R"))
}

// ---------------------------------------------------------------------------------------
// gen/encrypted-rc4-40.pdf
// ---------------------------------------------------------------------------------------

const PAD: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];

/// Standard security handler, V=1 R=2, 40-bit RC4. User password `user`, owner `owner`,
/// `/P -21` (no print, no copy). PDFium can *read* this and reports `Revision2`; it cannot
/// write encryption at all (pages spike §5).
fn encrypted_rc4_40() -> Vec<u8> {
    const KEYLEN: usize = 5;
    let user_pw = b"user";
    let owner_pw = b"owner";
    let permissions: i32 = -21;
    let doc_id = md5(b"seepdf-fixture-2026");

    let pad_pw = |pw: &[u8]| -> Vec<u8> {
        let mut v = pw.to_vec();
        v.extend_from_slice(&PAD);
        v.truncate(32);
        v
    };

    // Algorithm 3: /O
    let o_key = md5(&pad_pw(owner_pw))[..KEYLEN].to_vec();
    let o_entry = rc4(&o_key, &pad_pw(user_pw));

    // Algorithm 2: the encryption key
    let mut seed = pad_pw(user_pw);
    seed.extend_from_slice(&o_entry);
    seed.extend_from_slice(&permissions.to_le_bytes());
    seed.extend_from_slice(&doc_id);
    let key = md5(&seed)[..KEYLEN].to_vec();

    // Algorithm 4: /U for R=2
    let u_entry = rc4(&key, &PAD);

    let object_key = |number: usize, generation: u16| -> Vec<u8> {
        let mut seed = key.clone();
        seed.extend_from_slice(&(number as u32).to_le_bytes()[..3]);
        seed.extend_from_slice(&generation.to_le_bytes()[..2]);
        md5(&seed)[..(KEYLEN + 5).min(16)].to_vec()
    };
    let hex = |b: &[u8]| -> String {
        let mut s = String::from("<");
        for byte in b {
            s.push_str(&format!("{byte:02x}"));
        }
        s.push('>');
        s
    };

    // Object numbers are fixed, because each string and stream is encrypted with a key
    // derived from its own object number.
    let content = b"BT /F1 18 Tf 40 120 Td (SeePDF encrypted fixture) Tj ET\n\
                    BT /F1 12 Tf 40 96 Td (user password: user) Tj ET\n";
    let enc_content = rc4(&object_key(4, 0), content);
    let enc_title = rc4(&object_key(7, 0), b"Encrypted SeePDF Fixture");
    let enc_author = rc4(&object_key(7, 0), b"SeePDF Stage 0");

    let mut pdf = Pdf::default();
    let catalog = pdf.add("<< /Type /Catalog /Pages 2 0 R >>");
    let pages = pdf.add("<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let page = pdf.add(
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>",
    );
    let contents = pdf.stream(&enc_content);
    let font = pdf.add("<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>");
    let encrypt = pdf.add(format!(
        "<< /Filter /Standard /V 1 /R 2 /O {} /U {} /P {permissions} >>",
        hex(&o_entry),
        hex(&u_entry)
    ));
    let info = pdf.add(format!(
        "<< /Title {} /Author {} >>",
        hex(&enc_title),
        hex(&enc_author)
    ));
    assert_eq!(
        (catalog, pages, page, contents, font, encrypt, info),
        (1, 2, 3, 4, 5, 6, 7),
        "object numbers are baked into the per-object encryption keys"
    );
    pdf.finish(&format!(
        "/Root 1 0 R /Info 7 0 R /Encrypt 6 0 R /ID [{} {}]",
        hex(&doc_id),
        hex(&doc_id)
    ))
}

fn rc4(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut s: [u8; 256] = std::array::from_fn(|i| i as u8);
    let mut j = 0u8;
    for i in 0..256usize {
        j = j.wrapping_add(s[i]).wrapping_add(key[i % key.len()]);
        s.swap(i, j as usize);
    }
    let (mut i, mut j) = (0u8, 0u8);
    data.iter()
        .map(|&c| {
            i = i.wrapping_add(1);
            j = j.wrapping_add(s[i as usize]);
            s.swap(i as usize, j as usize);
            c ^ s[(s[i as usize].wrapping_add(s[j as usize])) as usize]
        })
        .collect()
}

/// RFC 1321. Needed only by the fixture generator, which is why there is no `md5` crate in
/// `Cargo.toml`.
fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5,
        9, 14, 20, 5, 9, 14, 20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10,
        15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
    ];
    let k: [u32; 64] =
        std::array::from_fn(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32);

    let mut message = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_le_bytes());

    let (mut a0, mut b0, mut c0, mut d0) =
        (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in message.chunks_exact(64) {
        let m: [u32; 16] = std::array::from_fn(|i| {
            u32::from_le_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ])
        });
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f
                .wrapping_add(a)
                .wrapping_add(k[i])
                .wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&a0.to_le_bytes());
    out[4..8].copy_from_slice(&b0.to_le_bytes());
    out[8..12].copy_from_slice(&c0.to_le_bytes());
    out[12..16].copy_from_slice(&d0.to_le_bytes());
    out
}

// ---------------------------------------------------------------------------------------
// gen/korean-300dpi.pdf
// ---------------------------------------------------------------------------------------

/// A sparse Korean page, sized so a 300 DPI render is what the OCR module recognises.
/// Needs a Hangul TrueType font to embed, because PDFium's base-14 fonts cannot draw 한글.
fn korean_page() -> Result<Vec<u8>, String> {
    use pdfium_render::prelude::*;

    // Only the bundled subset is acceptable: PDFium embeds the *whole* font file with no
    // subsetting, so a system font produces a 6-31 MB fixture (text spike §5) — Arial
    // Unicode gives 15.3 MB. Stage 1 (b) builds `SeePDF-Hangul.ttf`
    // (`examples/build_hangul_font.rs`); until then this fixture is skipped.
    let font_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("fonts")
        .join("SeePDF-Hangul.ttf");
    if !font_path.is_file() {
        return Err(format!(
            "{} is missing (Stage 1 (b) builds it); a system Hangul font would embed 6-31 MB",
            font_path.display()
        ));
    }

    let (library, _) = seepdf_lib::app::pdfium_path::resolve_local()?;
    let pdfium = Pdfium::new(
        Pdfium::bind_to_library(&library).map_err(|e| format!("bind libpdfium: {e:?}"))?,
    );
    let mut doc = pdfium
        .create_new_pdf()
        .map_err(|e| format!("create_new_pdf: {e:?}"))?;
    let font_bytes = std::fs::read(&font_path).map_err(|e| format!("read font: {e}"))?;
    let token = doc
        .fonts_mut()
        .load_true_type_from_bytes(&font_bytes, true)
        .map_err(|e| format!("load font: {e:?}"))?;

    let lines = [
        ("검색 가능한 한글 문서", 24.0f32),
        ("SeePDF OCR 정확도 측정용 고정 페이지입니다.", 14.0),
        ("이 문장은 300 DPI 로 렌더링하면 또렷하게 읽힙니다.", 14.0),
        ("Mixed script: SeePDF 2026 version 1.0", 14.0),
    ];
    {
        let mut page = doc
            .pages_mut()
            .create_page_at_end(PdfPagePaperSize::a4())
            .map_err(|e| format!("create page: {e:?}"))?;
        page.set_content_regeneration_strategy(PdfPageContentRegenerationStrategy::Manual);
        let mut y = 720.0f32;
        for (text, size) in lines {
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(64.0),
                    PdfPoints::new(y),
                    text,
                    token,
                    PdfPoints::new(size),
                )
                .map_err(|e| format!("create_text_object: {e:?}"))?;
            y -= size * 2.2;
        }
        page.regenerate_content()
            .map_err(|e| format!("regenerate_content: {e:?}"))?;
    }
    println!("korean-300dpi.pdf embeds {}", font_path.display());
    doc.save_to_bytes()
        .map_err(|e| format!("save_to_bytes: {e:?}"))
}
