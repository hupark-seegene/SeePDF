//! Builds the committed `src-tauri/resources/fonts/SeePDF-Hangul.ttf` — run once, by hand.
//!
//! ```sh
//! cd src-tauri
//! cargo run --release --example build_hangul_font -- /path/to/NotoSansKR-Variable.ttf
//! ```
//!
//! # Why this exists
//!
//! PDFium's `FPDFText_LoadFont` embeds the **whole** font file with no subsetting (text spike
//! §5): AppleGothic adds 6.2 MB to every document, AppleSDGothicNeo 31 MB. So SeePDF ships
//! one small OFL Hangul font and embeds that. Nothing subsets at runtime — this example is a
//! development tool and `subsetter` is a dev-dependency only.
//!
//! # Why the font is rebuilt rather than just subset
//!
//! `subsetter` targets PDF writers that supply their own CMap, so it **deletes the `cmap`
//! table** ("CID fonts in PDF define their own cmaps"). PDFium does the opposite: it maps the
//! string's code points to glyph ids through the *font's* `cmap`, and reverse-maps the same
//! table to generate `/ToUnicode`. A font with no `cmap` renders nothing at all. So this
//! example subsets the outlines with `subsetter` and then writes a fresh container with:
//!
//! * a **`cmap`** (format 4, referenced from the `(3,1)` and `(0,3)` encoding records) built
//!   from our own code point → new glyph id map;
//! * a **`name`** table naming the result `SeePDF Hangul`, keeping the original copyright and
//!   licence records (OFL requires it) — the name is what `PageObject.fontName` reports and
//!   what `TextEditProbe.substituteFont` promises;
//! * the original **`OS/2`** table, which `subsetter` drops and FreeType likes to have.
//!
//! # The AppleGothic bug this must not reproduce
//!
//! AppleGothic's generated `/ToUnicode` maps the space glyph to U+0009, so extracted Korean
//! text comes back as `한글\t테스트` and search for `한글 테스트` fails (text spike §5,
//! `WORKPLAN.md` §6). The cause is a many-to-one `cmap`: several code points share one glyph
//! and PDFium's reverse map picks the wrong one. [`build_cmap`] therefore keeps **one code
//! point per glyph id** (the lowest, with the ASCII range winning ties), so U+0020 is the only
//! thing that maps to the space glyph. `ocr_layer_apply_searchable` is the gate.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use subsetter::GlyphRemapper;

/// The family name written into the subset, reported by `PdfFont::name()` and used as
/// `TextEditProbe.substituteFont`.
const FAMILY: &str = "SeePDF Hangul";
const POSTSCRIPT: &str = "SeePDF-Hangul";

fn main() {
    if let Err(e) = run() {
        eprintln!("build_hangul_font: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let source = source_font()?;
    let data = std::fs::read(&source).map_err(|e| format!("read {}: {e}", source.display()))?;
    println!(
        "source: {} ({} KB)",
        source.display(),
        data.len() as u64 / 1024
    );

    let face = ttf_parser::Face::parse(&data, 0).map_err(|e| format!("parse source: {e}"))?;
    println!(
        "  {} glyphs, units_per_em {}, glyf: {}",
        face.number_of_glyphs(),
        face.units_per_em(),
        face.tables().glyf.is_some()
    );

    // ---- 1. which code points the subset must cover -------------------------------------
    let wanted = wanted_code_points();
    println!("  wanted code points: {}", wanted.len());

    // ---- 2. code point -> old glyph id, one code point per glyph -------------------------
    let mut by_glyph: BTreeMap<u16, u32> = BTreeMap::new();
    // Code points that will not be in the subset's cmap, and why.
    let mut no_glyph: Vec<u32> = Vec::new();
    let mut duplicate: Vec<u32> = Vec::new();
    for &cp in &wanted {
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        match face.glyph_index(ch) {
            Some(gid) if gid.0 != 0 => {
                // First writer wins: `wanted` is ascending, so ASCII beats the compatibility
                // duplicates further up (U+0020 beats U+00A0, U+2007, U+3000, the Hangul
                // filler U+3164 …). This is what keeps PDFium's generated /ToUnicode from
                // turning a space into a TAB.
                match by_glyph.entry(gid.0) {
                    std::collections::btree_map::Entry::Vacant(slot) => {
                        slot.insert(cp);
                    }
                    std::collections::btree_map::Entry::Occupied(_) => duplicate.push(cp),
                }
            }
            _ => no_glyph.push(cp),
        }
    }
    report("have no glyph in the source font", &no_glyph);
    report("share a glyph with an earlier code point", &duplicate);
    let mut cmap_pairs: Vec<(u32, u16)> = by_glyph.iter().map(|(&gid, &cp)| (cp, gid)).collect();
    cmap_pairs.sort_unstable();
    println!("  glyphs kept: {}", cmap_pairs.len());

    // ---- 3. subset the outlines ---------------------------------------------------------
    let old_gids: Vec<u16> = {
        let mut v: Vec<u16> = cmap_pairs.iter().map(|&(_, g)| g).collect();
        v.sort_unstable();
        v
    };
    let mut remapper = GlyphRemapper::new();
    for gid in &old_gids {
        remapper.remap(*gid);
    }
    // A variable font's `glyf` holds the **default** instance, and Noto Sans KR's `wght` axis
    // defaults to 100 (Thin) — subsetting it as-is would ship a hairline font. `subsetter`
    // instantiates through skrifa when variation coordinates are given.
    let subset = match weight_axis(&face) {
        Some((min, _, max)) => {
            let wght = 400.0f32.clamp(min, max);
            println!("  variable source: instancing at wght {wght}");
            subsetter::subset_with_variations(
                &data,
                0,
                &[(subsetter::Tag::new(b"wght"), wght)],
                &remapper,
            )
        }
        None => subsetter::subset(&data, 0, &remapper),
    }
    .map_err(|e| format!("subsetter::subset: {e:?}"))?;
    println!("  subsetter output: {} KB", subset.len() as u64 / 1024);

    // `subsetter` pulls in composite-glyph components, so the remapper may hold more glyphs
    // than we asked for; only the ones we mapped get a cmap entry.
    let new_pairs: Vec<(u32, u16)> = cmap_pairs
        .iter()
        .filter_map(|&(cp, old)| remapper.get(old).map(|new| (cp, new)))
        .collect();
    if new_pairs.len() != cmap_pairs.len() {
        return Err(format!(
            "remapper lost {} glyphs",
            cmap_pairs.len() - new_pairs.len()
        ));
    }

    // ---- 4. rebuild the container with cmap + name + OS/2 --------------------------------
    let cmap = build_cmap(&new_pairs);
    let name = build_name(&face);
    let os2 = face
        .raw_face()
        .table(ttf_parser::Tag::from_bytes(b"OS/2"))
        .map(|t| t.to_vec());
    let font = rebuild(&subset, cmap, name, os2)?;

    // ---- 5. verify -----------------------------------------------------------------------
    let mut absent = no_glyph;
    absent.extend(duplicate);
    verify(&font, &wanted, &absent)?;

    // ---- 6. write ------------------------------------------------------------------------
    let out_dir = resources_fonts_dir();
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("create {}: {e}", out_dir.display()))?;
    let out = out_dir.join("SeePDF-Hangul.ttf");
    std::fs::write(&out, &font).map_err(|e| format!("write {}: {e}", out.display()))?;
    println!(
        "wrote {} ({} bytes, {:.2} MB)",
        out.display(),
        font.len(),
        font.len() as f64 / (1024.0 * 1024.0)
    );

    copy_licence(&source, &out_dir)?;
    Ok(())
}

/// One line per class of code point that did not make it into the subset.
fn report(why: &str, cps: &[u32]) {
    if cps.is_empty() {
        return;
    }
    println!(
        "  {} code points {why} and are dropped (first few: {})",
        cps.len(),
        cps.iter()
            .take(8)
            .map(|c| format!("U+{c:04X}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
}

/// `(min, default, max)` of the `wght` axis when `face` is a variable font.
fn weight_axis(face: &ttf_parser::Face<'_>) -> Option<(f32, f32, f32)> {
    face.variation_axes()
        .into_iter()
        .find(|a| a.tag == ttf_parser::Tag::from_bytes(b"wght"))
        .map(|a| (a.min_value, a.def_value, a.max_value))
}

// ---------------------------------------------------------------------------------------
// The coverage set
// ---------------------------------------------------------------------------------------

/// Every code point the bundled font must be able to draw.
///
/// Ascending, deduplicated — the order matters, see the `by_glyph` comment in [`run`].
fn wanted_code_points() -> Vec<u32> {
    let mut set: BTreeSet<u32> = BTreeSet::new();

    // ASCII printable (0x20 space first: it must own the space glyph).
    for cp in 0x20u32..=0x7E {
        set.insert(cp);
    }
    // Latin-1 supplement, minus the soft hyphen and NBSP — both are usually the same glyph as
    // '-' and ' ' and would poison the reverse /ToUnicode map.
    for cp in 0xA1u32..=0xFF {
        if cp != 0xAD {
            set.insert(cp);
        }
    }
    // Hangul Jamo (conjoining) and Hangul Compatibility Jamo.
    for cp in 0x1100u32..=0x11FF {
        set.insert(cp);
    }
    for cp in 0x3130u32..=0x318F {
        set.insert(cp);
    }
    // Common punctuation, arrows and symbols Korean documents use.
    for cp in [
        0x2010u32, 0x2013, 0x2014, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2026, 0x2030, 0x2032,
        0x2033, 0x203B, 0x2116, 0x2122, 0x2190, 0x2191, 0x2192, 0x2193, 0x2194, 0x2195, 0x21D2,
        0x2200, 0x2202, 0x2203, 0x2206, 0x2207, 0x2208, 0x220F, 0x2211, 0x221A, 0x221D, 0x221E,
        0x2220, 0x2225, 0x2227, 0x2228, 0x2229, 0x222A, 0x222B, 0x222C, 0x2234, 0x2235, 0x223D,
        0x2252, 0x2260, 0x2261, 0x2264, 0x2265, 0x226A, 0x226B, 0x2282, 0x2283, 0x2286, 0x2287,
        0x2312, 0x2500, 0x2502, 0x25A0, 0x25A1, 0x25B2, 0x25B3, 0x25BC, 0x25BD, 0x25C6, 0x25C7,
        0x25CB, 0x25CE, 0x25CF, 0x2605, 0x2606, 0x261C, 0x261E, 0x2640, 0x2642, 0x2660, 0x2663,
        0x2665, 0x2666, 0x266A, 0x266D, 0x20A9, 0x20AC,
    ] {
        set.insert(cp);
    }
    // CJK symbols and punctuation (。、「」『』【】〜 …) and the ideographic space.
    for cp in 0x3001u32..=0x303F {
        set.insert(cp);
    }
    // Fullwidth forms: parentheses, digits and Latin letters appear constantly in scanned
    // Korean forms.
    for cp in 0xFF01u32..=0xFF5E {
        set.insert(cp);
    }
    // KS X 1001's 2,350 precomposed Hangul syllables — the ones a Korean keyboard produces.
    for cp in ksx1001_hangul() {
        set.insert(cp);
    }
    set.into_iter().collect()
}

/// The 2,350 Hangul syllables of KS X 1001 (the "완성형" set) — the table lives in the library
/// (`engine::fonts::ksx1001`) so the engine's own coverage test uses exactly the same list.
fn ksx1001_hangul() -> Vec<u32> {
    seepdf_lib::engine::fonts::ksx1001::syllables()
}

// ---------------------------------------------------------------------------------------
// cmap
// ---------------------------------------------------------------------------------------

/// A `cmap` with a format 4 (BMP) subtable, referenced from both the Windows Unicode BMP
/// `(3, 1)` and the Unicode `(0, 3)` encoding records, which is what FreeType — and therefore
/// PDFium — looks for.
///
/// `pairs` must be sorted by code point and hold **one code point per glyph id**.
fn build_cmap(pairs: &[(u32, u16)]) -> Vec<u8> {
    let bmp: Vec<(u16, u16)> = pairs
        .iter()
        .filter(|&&(cp, _)| cp <= 0xFFFF)
        .map(|&(cp, gid)| (cp as u16, gid))
        .collect();

    // Contiguous runs of code points become one segment each.
    let mut segments: Vec<(u16, u16, usize)> = Vec::new(); // (start, end, index into `bmp`)
    let mut i = 0usize;
    while i < bmp.len() {
        let start = bmp[i].0;
        let mut end = start;
        let first = i;
        i += 1;
        while i < bmp.len() && bmp[i].0 == end + 1 {
            end = bmp[i].0;
            i += 1;
        }
        segments.push((start, end, first));
    }

    let seg_count = segments.len() + 1; // + the mandatory 0xFFFF guard segment
    let mut end_codes = Vec::with_capacity(seg_count);
    let mut start_codes = Vec::with_capacity(seg_count);
    let mut id_deltas: Vec<i16> = Vec::with_capacity(seg_count);
    let mut id_range_offsets = Vec::with_capacity(seg_count);
    let mut glyph_array: Vec<u16> = Vec::new();

    for (index, &(start, end, first)) in segments.iter().enumerate() {
        start_codes.push(start);
        end_codes.push(end);
        id_deltas.push(0);
        // glyphIndexAddress = &idRangeOffset[i] + idRangeOffset[i] + 2 * (c - startCode[i])
        let offset = 2 * (seg_count - index) + 2 * glyph_array.len();
        id_range_offsets.push(offset as u16);
        for k in 0..=(end - start) as usize {
            glyph_array.push(bmp[first + k].1);
        }
    }
    start_codes.push(0xFFFF);
    end_codes.push(0xFFFF);
    id_deltas.push(1);
    id_range_offsets.push(0);

    let mut sub = Vec::new();
    let length = 16 + seg_count * 8 + glyph_array.len() * 2;
    let entry_selector = (seg_count as f64).log2().floor() as u16;
    let search_range = 2u16.pow(u32::from(entry_selector)) * 2;
    sub.extend_from_slice(&4u16.to_be_bytes());
    sub.extend_from_slice(&(length as u16).to_be_bytes());
    sub.extend_from_slice(&0u16.to_be_bytes()); // language
    sub.extend_from_slice(&((seg_count * 2) as u16).to_be_bytes());
    sub.extend_from_slice(&search_range.to_be_bytes());
    sub.extend_from_slice(&entry_selector.to_be_bytes());
    sub.extend_from_slice(&((seg_count as u16) * 2 - search_range).to_be_bytes());
    for c in &end_codes {
        sub.extend_from_slice(&c.to_be_bytes());
    }
    sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
    for c in &start_codes {
        sub.extend_from_slice(&c.to_be_bytes());
    }
    for d in &id_deltas {
        sub.extend_from_slice(&d.to_be_bytes());
    }
    for o in &id_range_offsets {
        sub.extend_from_slice(&o.to_be_bytes());
    }
    for g in &glyph_array {
        sub.extend_from_slice(&g.to_be_bytes());
    }
    debug_assert_eq!(sub.len(), length);

    let mut out = Vec::new();
    let n_records = 2u16;
    out.extend_from_slice(&0u16.to_be_bytes()); // version
    out.extend_from_slice(&n_records.to_be_bytes());
    let sub_offset = 4u32 + u32::from(n_records) * 8;
    for (platform, encoding) in [(3u16, 1u16), (0u16, 3u16)] {
        out.extend_from_slice(&platform.to_be_bytes());
        out.extend_from_slice(&encoding.to_be_bytes());
        out.extend_from_slice(&sub_offset.to_be_bytes());
    }
    out.extend_from_slice(&sub);
    out
}

// ---------------------------------------------------------------------------------------
// name
// ---------------------------------------------------------------------------------------

/// A format-0 `name` table that renames the face to `SeePDF Hangul` and **keeps the source
/// font's copyright, licence and licence URL records** — SIL OFL §1 requires it.
fn build_name(face: &ttf_parser::Face<'_>) -> Vec<u8> {
    let source_name = |id: u16| -> Option<String> {
        face.names()
            .into_iter()
            .find(|n| n.name_id == id && n.is_unicode())
            .and_then(|n| n.to_string())
            .or_else(|| {
                face.names()
                    .into_iter()
                    .find(|n| n.name_id == id)
                    .and_then(|n| n.to_string())
            })
    };
    let copyright = source_name(0)
        .unwrap_or_else(|| "Copyright of the original font, licensed under the SIL OFL".into());
    let licence = source_name(13).unwrap_or_else(|| {
        "This font is licensed under the SIL Open Font License, Version 1.1.".into()
    });
    let licence_url = source_name(14).unwrap_or_else(|| "https://scripts.sil.org/OFL".to_string());
    // The **family** name, not name ID 4: for a variable font ID 4 is the default instance's
    // full name, which for Noto Sans KR is "Noto Sans KR Thin" — and this subset is instanced at
    // wght 400, so saying "Thin" would be wrong.
    let origin = source_name(16)
        .or_else(|| source_name(1))
        .unwrap_or_else(|| "unknown".into());

    let entries: Vec<(u16, String)> = vec![
        (0, copyright),
        (1, FAMILY.to_string()),
        (2, "Regular".to_string()),
        (3, format!("{FAMILY}; SeePDF subset of {origin}")),
        (4, FAMILY.to_string()),
        (5, "Version 1.000".to_string()),
        (6, POSTSCRIPT.to_string()),
        (13, licence),
        (14, licence_url),
    ];

    // (platform, encoding, language) pairs, in the order the spec wants the records sorted.
    let mut records: Vec<(u16, u16, u16, u16, Vec<u8>)> = Vec::new();
    for (id, value) in &entries {
        // Macintosh / Roman / English, single byte.
        records.push((
            1,
            0,
            0,
            *id,
            value
                .chars()
                .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
                .collect(),
        ));
    }
    for (id, value) in &entries {
        // Windows / Unicode BMP / en-US, UTF-16BE.
        let mut utf16 = Vec::new();
        for unit in value.encode_utf16() {
            utf16.extend_from_slice(&unit.to_be_bytes());
        }
        records.push((3, 1, 0x0409, *id, utf16));
    }
    records.sort_by_key(|r| (r.0, r.1, r.2, r.3));

    let count = records.len();
    let storage_offset = 6 + 12 * count;
    let mut storage: Vec<u8> = Vec::new();
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes()); // format 0
    out.extend_from_slice(&(count as u16).to_be_bytes());
    out.extend_from_slice(&(storage_offset as u16).to_be_bytes());
    for (platform, encoding, language, id, value) in &records {
        out.extend_from_slice(&platform.to_be_bytes());
        out.extend_from_slice(&encoding.to_be_bytes());
        out.extend_from_slice(&language.to_be_bytes());
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(&(storage.len() as u16).to_be_bytes());
        storage.extend_from_slice(value);
    }
    out.extend_from_slice(&storage);
    out
}

// ---------------------------------------------------------------------------------------
// Container
// ---------------------------------------------------------------------------------------

/// Writes a fresh sfnt from `subset`'s tables plus (or replacing) `cmap`, `name` and `OS/2`,
/// with every table checksum and `head.checkSumAdjustment` recomputed.
fn rebuild(
    subset: &[u8],
    cmap: Vec<u8>,
    name: Vec<u8>,
    os2: Option<Vec<u8>>,
) -> Result<Vec<u8>, String> {
    let mut tables: BTreeMap<[u8; 4], Vec<u8>> = BTreeMap::new();
    if subset.len() < 12 {
        return Err("subset output is too small".into());
    }
    let sfnt = u32::from_be_bytes([subset[0], subset[1], subset[2], subset[3]]);
    let count = u16::from_be_bytes([subset[4], subset[5]]) as usize;
    for i in 0..count {
        let rec = 12 + i * 16;
        let end = rec + 16;
        if end > subset.len() {
            return Err("truncated table directory".into());
        }
        let mut tag = [0u8; 4];
        tag.copy_from_slice(&subset[rec..rec + 4]);
        let offset = u32::from_be_bytes(subset[rec + 8..rec + 12].try_into().unwrap()) as usize;
        let length = u32::from_be_bytes(subset[rec + 12..end].try_into().unwrap()) as usize;
        let data = subset
            .get(offset..offset + length)
            .ok_or_else(|| format!("table {} out of range", String::from_utf8_lossy(&tag)))?;
        tables.insert(tag, data.to_vec());
    }
    tables.insert(*b"cmap", cmap);
    tables.insert(*b"name", name);
    if let Some(os2) = os2 {
        tables.insert(*b"OS/2", os2);
    }

    let n = tables.len();
    let entry_selector = (n as f64).log2().floor() as u16;
    let search_range = 2u16.pow(u32::from(entry_selector)) * 16;
    let range_shift = (n as u16) * 16 - search_range;

    let mut out = Vec::new();
    out.extend_from_slice(&sfnt.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());

    let mut offset = 12 + n * 16;
    let mut head_checksum_adjust_at = None;
    for (tag, data) in &mut tables {
        if tag == b"head" {
            if data.len() < 12 {
                return Err("head table is too small".into());
            }
            data[8..12].fill(0);
            head_checksum_adjust_at = Some(offset + 8);
        }
        out.extend_from_slice(tag);
        out.extend_from_slice(&checksum(data).to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        offset += (data.len() + 3) & !3;
    }
    for data in tables.values() {
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    if let Some(at) = head_checksum_adjust_at {
        let total = checksum(&out);
        let adjustment = 0xB1B0_AFBAu32.wrapping_sub(total);
        out[at..at + 4].copy_from_slice(&adjustment.to_be_bytes());
    }
    Ok(out)
}

/// The sfnt checksum: the sum of the data as big-endian `u32`s, zero-padded to a multiple of 4.
fn checksum(data: &[u8]) -> u32 {
    let mut sum = 0u32;
    let mut chunks = data.chunks_exact(4);
    for c in &mut chunks {
        sum = sum.wrapping_add(u32::from_be_bytes([c[0], c[1], c[2], c[3]]));
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut last = [0u8; 4];
        last[..rest.len()].copy_from_slice(rest);
        sum = sum.wrapping_add(u32::from_be_bytes(last));
    }
    sum
}

// ---------------------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------------------

/// Re-parses the finished font with `ttf-parser` and proves the three things the rest of the
/// project depends on: every wanted code point resolves, every glyph has an outline (or is a
/// space), and **no two code points share a glyph** — the AppleGothic space→TAB bug.
fn verify(font: &[u8], wanted: &[u32], known_missing: &[u32]) -> Result<(), String> {
    let face = ttf_parser::Face::parse(font, 0).map_err(|e| format!("parse result: {e}"))?;
    println!(
        "verify: {} glyphs, units_per_em {}, name {:?}",
        face.number_of_glyphs(),
        face.units_per_em(),
        face.names()
            .into_iter()
            .find(|n| n.name_id == 1 && n.is_unicode())
            .and_then(|n| n.to_string())
    );

    let missing: BTreeSet<u32> = known_missing.iter().copied().collect();
    let mut seen: BTreeMap<u16, u32> = BTreeMap::new();
    let mut unresolved = Vec::new();
    let mut outline_less = Vec::new();
    for &cp in wanted {
        if missing.contains(&cp) {
            continue;
        }
        let Some(ch) = char::from_u32(cp) else {
            continue;
        };
        let Some(gid) = face.glyph_index(ch) else {
            unresolved.push(cp);
            continue;
        };
        if gid.0 == 0 {
            unresolved.push(cp);
            continue;
        }
        if let Some(previous) = seen.insert(gid.0, cp) {
            return Err(format!(
                "U+{cp:04X} and U+{previous:04X} share glyph {} — PDFium's generated \
                 /ToUnicode would reverse-map one of them to the other (the AppleGothic \
                 space→TAB bug)",
                gid.0
            ));
        }
        // A space legitimately has no outline; everything else must have one.
        if cp != 0x20 && face.glyph_bounding_box(gid).is_none() {
            outline_less.push(cp);
        }
    }
    if !unresolved.is_empty() {
        return Err(format!(
            "{} code points do not resolve in the subset (first: U+{:04X})",
            unresolved.len(),
            unresolved[0]
        ));
    }
    if !outline_less.is_empty() {
        println!(
            "  note: {} code points have no outline (blank glyphs): {}",
            outline_less.len(),
            outline_less
                .iter()
                .take(8)
                .map(|c| format!("U+{c:04X}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    let space = face
        .glyph_index(' ')
        .ok_or_else(|| "the subset has no space glyph".to_string())?;
    let advance = face
        .glyph_hor_advance(space)
        .ok_or_else(|| "the space glyph has no advance width".to_string())?;
    if advance == 0 {
        return Err(
            "the space glyph has advance width 0 — text would render as SeePDFedited".into(),
        );
    }
    println!(
        "  space = glyph {} with advance {} ({} code points verified, no glyph shared)",
        space.0,
        advance,
        seen.len()
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------------------

fn resources_fonts_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("fonts")
}

/// `--font <path>` / the first positional argument / `$SEEPDF_HANGUL_SRC` / a few defaults.
fn source_font() -> Result<PathBuf, String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--font" {
            if let Some(path) = args.next() {
                return Ok(PathBuf::from(path));
            }
        } else if !arg.starts_with("--") {
            return Ok(PathBuf::from(arg));
        }
    }
    if let Ok(path) = std::env::var("SEEPDF_HANGUL_SRC") {
        return Ok(PathBuf::from(path));
    }
    let defaults = [
        resources_fonts_dir().join("_src/NanumGothic-Regular.ttf"),
        resources_fonts_dir().join("_src/NotoSansKR-Variable.ttf"),
    ];
    for candidate in defaults {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(
        "no source font. Pass one: `cargo run --release --example build_hangul_font -- \
         /path/to/NanumGothic-Regular.ttf` (any OFL Korean TTF works; NanumGothic-Regular is \
         what the committed subset was built from)."
            .into(),
    )
}

/// Copies the source font's OFL text next to the subset, as the licence requires.
fn copy_licence(source: &Path, out_dir: &Path) -> Result<(), String> {
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let family = stem.split('-').next().unwrap_or("the source font");
    let candidates: Vec<PathBuf> = source
        .parent()
        .map(|dir| {
            vec![
                dir.join(format!("OFL-{family}.txt")),
                dir.join("OFL.txt"),
                dir.join("LICENSE.txt"),
                dir.join("OFL-NanumGothic.txt"),
            ]
        })
        .unwrap_or_default();
    let Some(found) = candidates.into_iter().find(|p| p.is_file()) else {
        println!(
            "note: no OFL text found next to {} — copy it into {} by hand",
            source.display(),
            out_dir.display()
        );
        return Ok(());
    };
    let body = std::fs::read_to_string(&found).map_err(|e| format!("read licence: {e}"))?;
    let header = format!(
        "SeePDF-Hangul.ttf is a subset of {stem}, redistributed under the SIL Open Font\n\
         License 1.1 reproduced below. Rebuild it with\n\
         `cargo run --release --example build_hangul_font -- <path to {stem}.ttf>`.\n\n\
         ----------------------------------------------------------------------------\n\n"
    );
    let out = out_dir.join("OFL.txt");
    std::fs::write(&out, format!("{header}{body}"))
        .map_err(|e| format!("write {}: {e}", out.display()))?;
    println!("wrote {}", out.display());
    Ok(())
}
