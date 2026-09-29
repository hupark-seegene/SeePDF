//! P2 — annotation summary export (`export_annotation_summary`, `IPC_CONTRACT.md` §7.7a).
//!
//! A tracemonkey copy gets annotations on pages 4, 1, 2 (created out of page order, the note
//! with Korean text, a comma and quotes); the export must list them page by page, quote the
//! highlighted words from the text layer, and write CSV that Excel reads as UTF-8.

mod common;
use common::*;

use seepdf_lib::engine::annot;
use seepdf_lib::engine::export::summary;
use seepdf_lib::engine::registry::{self, MutateOpts};
use seepdf_lib::engine::text::layer;
use seepdf_lib::ipc::types::{
    AnnotSpec, ChangeReason, InkSpec, Locale, MarkupSpec, NoteSpec, Rect, ShapeSpec, SummaryFormat,
};
use seepdf_lib::ipc::ErrorCode;
use std::path::PathBuf;

fn out_dir() -> PathBuf {
    let dir = fixture("out").join("p2-summary");
    std::fs::create_dir_all(&dir).expect("create fixtures/out/p2-summary");
    dir
}

fn create(doc_id: &str, page: u16, spec: AnnotSpec) {
    let doc_id = doc_id.to_string();
    with_state(move |st| {
        registry::mutate(
            st,
            &doc_id,
            MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(page),
            |doc| annot::create::create(doc, page, &spec, None),
        )
    })
    .expect("create annotation");
}

/// The line rects of the first occurrence of `needle` on `page`, from the text layer.
fn rects_of(doc_id: &str, page: u16, needle: &'static str) -> Vec<Rect> {
    with_doc(doc_id, move |d| {
        let l = layer::layer(d, page)?;
        let chars: Vec<char> = l.text.chars().collect();
        let n: Vec<char> = needle.chars().collect();
        let start = chars
            .windows(n.len())
            .position(|w| w == n.as_slice())
            .expect("needle on the page");
        Ok(l.range_rects(start as u32, n.len() as u32))
    })
    .unwrap()
}

fn export(
    doc_id: &str,
    name: &str,
    format: SummaryFormat,
    pages: Option<Vec<u16>>,
) -> (PathBuf, u32) {
    let path = out_dir().join(name);
    let _ = std::fs::remove_file(&path);
    let doc_id = doc_id.to_string();
    let target = path.display().to_string();
    let result = with_state(move |st| {
        summary::export_annotation_summary(
            st,
            &doc_id,
            &target,
            format,
            pages.as_deref(),
            Locale::Ko,
        )
    })
    .expect("export_annotation_summary");
    assert_eq!(result.bytes, std::fs::metadata(&path).unwrap().len());
    (path, result.count)
}

/// A minimal RFC 4180 reader: records of fields, quotes and embedded line breaks honoured.
fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') if field.is_empty() => quoted = true,
            (false, ',') => record.push(std::mem::take(&mut field)),
            (false, '\r') if chars.peek() == Some(&'\n') => {
                chars.next();
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

fn annotated() -> TestDoc {
    let doc = open("tracemonkey.pdf");
    // out of page order on purpose: page 4 first
    create(
        &doc.doc_id,
        3,
        AnnotSpec::Square(ShapeSpec {
            rect: Rect::new(100.0, 100.0, 200.0, 200.0),
            color: [0, 0, 255],
            fill_color: None,
            width: 2.0,
            opacity: 1.0,
        }),
    );
    let trace = rects_of(&doc.doc_id, 0, "Trace-based");
    create(
        &doc.doc_id,
        0,
        AnnotSpec::Highlight(MarkupSpec {
            rects: trace,
            color: [255, 212, 0],
            opacity: 0.6,
            contents: Some("=SUM(A1)".into()),
        }),
    );
    create(
        &doc.doc_id,
        0,
        AnnotSpec::Note(NoteSpec {
            at: [500.0, 700.0],
            color: [255, 200, 0],
            contents: "확인 필요, \"중요\"\n두 번째 줄".into(),
        }),
    );
    let words = rects_of(&doc.doc_id, 1, "trace");
    create(
        &doc.doc_id,
        1,
        AnnotSpec::Underline(MarkupSpec {
            rects: words,
            color: [0, 160, 0],
            opacity: 1.0,
            contents: None,
        }),
    );
    create(
        &doc.doc_id,
        1,
        AnnotSpec::Ink(InkSpec {
            paths: vec![vec![100.0, 100.0, 150.0, 140.0, 200.0, 110.0]],
            color: [200, 0, 0],
            width: 2.0,
            opacity: 1.0,
        }),
    );
    doc
}

#[test]
fn summary_csv_one_row_per_annotation_in_page_order() {
    let doc = annotated();
    let (path, count) = export(&doc.doc_id, "summary.csv", SummaryFormat::Csv, None);
    assert_eq!(count, 5);
    let bytes = std::fs::read(&path).unwrap();
    assert_eq!(&bytes[..3], &[0xEF, 0xBB, 0xBF], "UTF-8 BOM for Excel");
    let text = String::from_utf8(bytes[3..].to_vec()).expect("UTF-8");
    let records = parse_csv(&text);
    assert_eq!(
        records.len(),
        1 + 5,
        "header + one row per annotation: {records:?}"
    );
    assert_eq!(
        records[0],
        [
            "페이지",
            "페이지 레이블",
            "종류",
            "작성자",
            "만든 날짜",
            "수정한 날짜",
            "색상",
            "내용",
            "인용 텍스트",
            "ID",
            "답글 대상"
        ]
    );
    let pages: Vec<&str> = records[1..].iter().map(|r| r[0].as_str()).collect();
    assert_eq!(pages, ["1", "1", "2", "2", "4"], "page order");
    for r in &records[1..] {
        assert_eq!(r.len(), 11, "every row has every column: {r:?}");
    }
    let highlight = records
        .iter()
        .find(|r| r[2] == "형광펜")
        .expect("highlight row");
    assert_eq!(highlight[8], "Trace-based", "quoted from the text layer");
    assert_eq!(highlight[6], "#FFD400");
    assert_eq!(highlight[7], "'=SUM(A1)", "formula guard");
    let note = records.iter().find(|r| r[2] == "메모").expect("note row");
    assert_eq!(
        note[7], "확인 필요, \"중요\"\n두 번째 줄",
        "quotes, comma and line break survive"
    );
    assert!(note[8].is_empty());
    let underline = records
        .iter()
        .find(|r| r[2] == "밑줄")
        .expect("underline row");
    assert_eq!(underline[8].to_lowercase(), "trace");
    assert!(records.iter().any(|r| r[2] == "펜" && r[0] == "2"));
    assert!(records.iter().any(|r| r[2] == "사각형" && r[0] == "4"));
    // the dates are formatted, not raw PDF dates
    assert!(
        !highlight[5].starts_with("D:") && highlight[5].len() == 19,
        "{}",
        highlight[5]
    );
    // the raw text: quoted field with the doubled quotes, CRLF record ends
    assert!(text.contains("\"확인 필요, \"\"중요\"\"\n두 번째 줄\""));
    assert!(text.ends_with("\r\n"));
}

#[test]
fn summary_txt_md_and_page_filter() {
    let doc = annotated();
    let (md, count) = export(&doc.doc_id, "summary.md", SummaryFormat::Md, None);
    assert_eq!(count, 5);
    let md = std::fs::read_to_string(md).unwrap();
    assert!(md.starts_with("# 주석 목록 — tracemonkey.pdf\n"), "{md}");
    let first = md.find("## 1쪽").expect("page 1 heading");
    let second = md.find("## 2쪽").expect("page 2 heading");
    let fourth = md.find("## 4쪽").expect("page 4 heading");
    assert!(first < second && second < fourth);
    assert!(md.contains("  > Trace-based"));

    let (txt, _) = export(&doc.doc_id, "summary.txt", SummaryFormat::Txt, None);
    let txt = std::fs::read_to_string(txt).unwrap();
    assert!(!txt.starts_with('\u{FEFF}'));
    assert!(txt.contains("[1쪽] 형광펜"), "{txt}");
    assert!(txt.contains("인용 텍스트: \u{201C}Trace-based\u{201D}"));

    // only page 2
    let (only, count) = export(&doc.doc_id, "page2.csv", SummaryFormat::Csv, Some(vec![1]));
    assert_eq!(count, 2);
    let text = String::from_utf8(std::fs::read(only).unwrap()[3..].to_vec()).unwrap();
    assert!(parse_csv(&text)[1..].iter().all(|r| r[0] == "2"));

    // a page outside the document is refused, nothing written
    let doc_id = doc.doc_id.clone();
    let path = out_dir().join("never.csv");
    let _ = std::fs::remove_file(&path);
    let target = path.display().to_string();
    let err = with_state(move |st| {
        summary::export_annotation_summary(
            st,
            &doc_id,
            &target,
            SummaryFormat::Csv,
            Some(&[99]),
            Locale::Ko,
        )
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidArgument);
    assert!(!path.exists());
}

/// A third-party file: its highlight is exported with the words under it and its kind.
#[test]
fn summary_reads_third_party_annotations() {
    let doc = open("annotation-highlight.pdf");
    let rows = with_state({
        let id = doc.doc_id.clone();
        move |st| summary::collect(st, &id, None, Locale::En)
    })
    .unwrap();
    assert!(!rows.is_empty(), "the fixture has annotations");
    assert!(rows.windows(2).all(|w| w[0].page <= w[1].page));
    let hl = rows
        .iter()
        .find(|r| r.kind == "Highlight")
        .expect("a highlight row");
    assert!(!hl.quote.is_empty(), "highlighted words are quoted: {hl:?}");
}

/// v0.3 A7: a parent with two replies — the replies nest under it in Markdown / TXT (oldest
/// first) and the CSV names their parent in `답글 대상`.
#[test]
fn summary_nests_reply_threads() {
    let doc = open("tracemonkey.pdf");
    let doc_id = doc.doc_id.clone();
    let parent = with_state({
        let doc_id = doc_id.clone();
        move |st| {
            registry::mutate(
                st,
                &doc_id,
                MutateOpts::new("undo.annotCreate", ChangeReason::Edit).page(0),
                |doc| {
                    annot::create::create(
                        doc,
                        0,
                        &AnnotSpec::Note(NoteSpec {
                            at: [100.0, 700.0],
                            color: [255, 200, 0],
                            contents: "원문 메모".into(),
                        }),
                        None,
                    )
                },
            )
        }
    })
    .expect("parent");
    for (text, who) in [("첫 번째 답글", "김철수"), ("두 번째 답글", "이영희")] {
        let (doc_id, parent) = (doc_id.clone(), parent.clone());
        with_state(move |st| annot::reply::reply(st, &doc_id, 0, &parent, text, Some(who)))
            .expect("reply");
        // Distinct creation seconds, so "oldest first" is observable.
        std::thread::sleep(std::time::Duration::from_millis(1100));
    }

    let (md, count) = export(&doc_id, "threads.md", SummaryFormat::Md, None);
    assert_eq!(count, 3);
    let md = std::fs::read_to_string(md).unwrap();
    let top = md.find("- **메모** · ").expect("parent item");
    let first = md
        .find("  - **메모** · 김철수")
        .expect("first reply nested");
    let second = md
        .find("  - **메모** · 이영희")
        .expect("second reply nested");
    assert!(top < first && first < second, "{md}");
    assert!(md.contains("    첫 번째 답글\n"), "{md}");

    let (txt, _) = export(&doc_id, "threads.txt", SummaryFormat::Txt, None);
    let txt = std::fs::read_to_string(txt).unwrap();
    assert!(txt.contains("\n    \u{21B3} 메모 · 김철수"), "{txt}");

    let (csv, _) = export(&doc_id, "threads.csv", SummaryFormat::Csv, None);
    let text = String::from_utf8(std::fs::read(csv).unwrap()[3..].to_vec()).unwrap();
    let records = parse_csv(&text);
    assert_eq!(records.len(), 4);
    let top_row = records
        .iter()
        .find(|r| r[7] == "원문 메모")
        .expect("parent row");
    assert_eq!(top_row[9], parent);
    assert!(top_row[10].is_empty(), "a top-level row replies to nothing");
    let replies: Vec<&Vec<String>> = records.iter().filter(|r| r[10] == parent).collect();
    assert_eq!(
        replies.len(),
        2,
        "both replies name their parent: {records:?}"
    );
    assert_eq!(replies[0][7], "첫 번째 답글");
}
