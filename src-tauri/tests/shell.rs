//! v0.3 pkg5-app-shell-release-diagnostics: the Rust side of the app shell items.
//!
//! * H10 — the rolling log writer, the log tail, the startup-failure message, 문제 보고 text;
//! * H3  — `open_document` says *why* a file did not open (`notPdf`, `corrupted`);
//! * H4  — a panicking engine command answers `engineCrashed`, closes its document, and the
//!   engine keeps serving the others;
//! * H2  — the second-instance argv → PDF paths;
//! * H11 — the notices reader and its fallback;
//! * H14 — the first-run window fits the work area.

mod common;
use common::*;

use seepdf_lib::app::diagnostics::{self, StartupFailure};
use seepdf_lib::engine::{registry, Lane};
use seepdf_lib::ipc::types::{AppInfo, Locale, Theme};
use seepdf_lib::ipc::ErrorCode;
use std::io::Write;
use std::path::{Path, PathBuf};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir()
        .join(format!("seepdf-shell-test-{}", std::process::id()))
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

// ---------------------------------------------------------------------------------------
// H10 — logs, startup failure, 문제 보고
// ---------------------------------------------------------------------------------------

#[test]
fn the_log_writer_creates_a_dated_file_under_the_given_dir() {
    let dir = temp_dir("logs").join("nested");
    let mut writer = diagnostics::file_appender(&dir).expect("appender");
    writeln!(writer, "first line").unwrap();
    writeln!(writer, "second line").unwrap();
    writer.flush().unwrap();
    let files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let name = &files[0];
    assert!(
        name.starts_with("seepdf.") && name.ends_with(".log"),
        "daily file name: {name}"
    );
    let text = std::fs::read_to_string(dir.join(name)).unwrap();
    assert_eq!(text, "first line\nsecond line\n");
}

#[test]
fn the_log_tail_spans_the_newest_files() {
    let dir = temp_dir("tail");
    let yesterday: String = (0..150).map(|i| format!("old {i}\n")).collect();
    let today: String = (0..120).map(|i| format!("new {i}\n")).collect();
    std::fs::write(dir.join("seepdf.2026-09-28.log"), yesterday).unwrap();
    std::fs::write(dir.join("seepdf.2026-09-29.log"), today).unwrap();
    std::fs::write(dir.join("problem-report.txt"), "not a log").unwrap();
    let lines = diagnostics::recent_log_lines(&dir, 200);
    assert_eq!(lines.len(), 200);
    assert_eq!(lines[0], "old 70", "80 of yesterday's lines, oldest first");
    assert_eq!(lines[79], "old 149");
    assert_eq!(lines[80], "new 0");
    assert_eq!(lines[199], "new 119");
    assert!(diagnostics::recent_log_lines(&temp_dir("tail-empty"), 200).is_empty());
}

#[test]
fn a_missing_library_gets_the_korean_reinstall_or_antivirus_message() {
    let failure = StartupFailure::Library(
        "libpdfium not found; looked in: C:\\Program Files\\SeePDF\\resources\\pdfium".into(),
    );
    let (title, body) =
        diagnostics::startup_failure_text(&failure, Locale::Ko, Some(Path::new("C:\\logs")));
    assert_eq!(title, "SeePDF를 시작할 수 없습니다");
    assert!(
        body.contains("라이브러리를 불러올 수 없습니다 — 재설치하거나 백신 예외를 추가하세요"),
        "{body}"
    );
    assert!(body.contains("오류: libpdfium not found"), "{body}");
    assert!(body.contains("로그: C:\\logs"), "{body}");

    let (title, body) = diagnostics::startup_failure_text(&failure, Locale::En, None);
    assert_eq!(title, "SeePDF cannot start");
    assert!(body.contains("antivirus exception"), "{body}");
    assert!(!body.contains("Log:"));

    let (_, other) = diagnostics::startup_failure_text(
        &StartupFailure::Other("WebView2 is missing".into()),
        Locale::Ko,
        None,
    );
    assert!(
        other.contains("SeePDF를 시작하는 중 문제가 발생했습니다"),
        "{other}"
    );
    assert!(other.contains("WebView2 is missing"));
}

#[test]
fn the_problem_report_names_the_build_and_carries_the_log_tail() {
    let info = AppInfo {
        version: "0.3.0".into(),
        os: "windows".into(),
        arch: "x86_64".into(),
        debug: false,
        pdfium_version: "155.0.8057.0".into(),
        pdfium_dir: "C:\\SeePDF\\resources\\pdfium".into(),
        locale: Locale::Ko,
        theme: Theme::System,
    };
    let lines = vec!["INFO a".to_string(), "ERROR b".to_string()];
    let text = diagnostics::problem_report(&info, &lines);
    assert!(text.starts_with("SeePDF 0.3.0\n"), "{text}");
    assert!(text.contains("OS: windows / x86_64"));
    assert!(text.contains("PDFium: 155.0.8057.0"));
    assert!(text.ends_with("INFO a\nERROR b\n"));

    let dir = temp_dir("report");
    let path = diagnostics::write_report(&dir, &text).unwrap();
    assert_eq!(path, dir.join(diagnostics::REPORT_FILE));
    assert_eq!(std::fs::read_to_string(path).unwrap(), text);
}

// ---------------------------------------------------------------------------------------
// H3 — why a file did not open
// ---------------------------------------------------------------------------------------

fn open_bytes(
    bytes: Vec<u8>,
) -> Result<seepdf_lib::ipc::types::DocInfo, seepdf_lib::ipc::EngineError> {
    engine().call_blocking(Lane::Edit, "test/open-bytes", move |st| {
        registry::open(st, Some(PathBuf::from("renamed.pdf")), bytes, None)
    })
}

#[test]
fn a_text_file_renamed_pdf_is_not_a_pdf() {
    let err = open_bytes("회의록\n이것은 텍스트 파일입니다.\n".as_bytes().to_vec()).unwrap_err();
    assert_eq!(err.code, ErrorCode::Pdfium, "{err:?}");
    assert_eq!(err.detail.as_deref(), Some("notPdf"));
    // An image renamed .pdf too.
    let err = open_bytes(b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec()).unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("notPdf"));
}

#[test]
fn a_pdf_header_with_a_broken_body_is_corrupted() {
    let err = open_bytes(b"%PDF-1.7\n1 0 obj << /Type /Catalog /Pages 9 0 R >>\n%%EOF\n".to_vec())
        .unwrap_err();
    assert_eq!(err.detail.as_deref(), Some("corrupted"), "{err:?}");
}

#[test]
fn a_real_pdf_opens_and_an_encrypted_one_keeps_its_password_code() {
    let doc = open("tracemonkey.pdf");
    assert!(doc.info.page_count > 0);
    let name = "gen/encrypted-rc4-40.pdf";
    if fixture(name).exists() {
        let err = try_open(name, None).err().expect("needs a password");
        assert_eq!(err.code, ErrorCode::PasswordRequired);
        assert_eq!(err.detail, None);
    }
}

#[test]
fn an_out_of_memory_read_is_tagged() {
    let err: seepdf_lib::ipc::EngineError =
        std::io::Error::from(std::io::ErrorKind::OutOfMemory).into();
    assert_eq!(err.detail.as_deref(), Some("outOfMemory"));
}

// ---------------------------------------------------------------------------------------
// H4 — engine-thread panic recovery
// ---------------------------------------------------------------------------------------

#[test]
fn a_panicking_command_answers_engine_crashed_and_the_engine_keeps_serving() {
    let victim = open("rotation.pdf");
    let bystander = open("tracemonkey.pdf");

    let victim_id = victim.doc_id.clone();
    let err = engine()
        .call_blocking(Lane::Edit, "test/panic", move |st| -> Result<(), _> {
            let doc = st.doc_mut(&victim_id)?;
            let _ = doc.page_count();
            panic!("simulated engine bug");
        })
        .unwrap_err();
    assert_eq!(err.code, ErrorCode::EngineCrashed, "{err:?}");
    assert!(
        err.message.contains("simulated engine bug"),
        "{}",
        err.message
    );

    // The document the command worked on was closed (its state may be half-edited)…
    let victim_id = victim.doc_id.clone();
    let gone = engine()
        .call_blocking(Lane::Interactive, "test/get", move |st| {
            st.doc(&victim_id).map(|d| d.page_count())
        })
        .unwrap_err();
    assert_eq!(gone.code, ErrorCode::NotFound);
    assert!(engine().shared.doc(&victim.doc_id).is_none());

    // …and the next command, on another document, succeeds on the same engine thread.
    let bystander_id = bystander.doc_id.clone();
    let chars = engine()
        .call_blocking(Lane::Interactive, "test/text", move |st| {
            let doc = st.doc_mut(&bystander_id)?;
            Ok(seepdf_lib::engine::text::layer::layer(doc, 0)?.chars.len())
        })
        .unwrap();
    assert!(chars > 0);
    // A new document still opens.
    let again = open("rotation.pdf");
    assert!(again.info.page_count > 0);
}

// ---------------------------------------------------------------------------------------
// H2 — the second instance's argv
// ---------------------------------------------------------------------------------------

#[test]
fn second_instance_args_resolve_against_its_own_working_directory() {
    let cwd = fixture("");
    let text = temp_dir("argv").join("notes.txt");
    std::fs::write(&text, "not a pdf").unwrap();
    let args = vec![
        "tracemonkey.pdf".to_string(), // relative → the second process's cwd
        fixture("rotation.pdf").display().to_string(), // absolute
        "--flag".to_string(),          // not a file
        "missing.pdf".to_string(),     // does not exist
        text.display().to_string(),    // not a pdf
    ];
    let paths = seepdf_lib::app::files::pdf_paths_from_args(args, &cwd);
    assert_eq!(
        paths,
        vec![cwd.join("tracemonkey.pdf"), fixture("rotation.pdf")]
    );
}

// ---------------------------------------------------------------------------------------
// H11 — 오픈 소스 라이선스
// ---------------------------------------------------------------------------------------

#[test]
fn notices_are_read_from_the_bundle_or_fall_back_to_the_shipped_licences() {
    let with = temp_dir("notices-with");
    std::fs::create_dir_all(with.join("notices")).unwrap();
    std::fs::write(
        with.join("notices/THIRD_PARTY_NOTICES.txt"),
        "pdfium-render MIT",
    )
    .unwrap();
    let without = temp_dir("notices-without");
    std::fs::create_dir_all(without.join("pdfium")).unwrap();
    std::fs::write(without.join("pdfium/LICENSE.pdfium"), "BSD text").unwrap();

    assert_eq!(
        seepdf_lib::commands::app::read_notices(&[without.clone(), with.clone()]).unwrap(),
        "pdfium-render MIT",
        "the generated file wins wherever it is"
    );
    let fallback = seepdf_lib::commands::app::read_notices(&[without]).unwrap();
    assert!(fallback.contains("==== PDFium ====") && fallback.contains("BSD text"));
    let none = seepdf_lib::commands::app::read_notices(&[temp_dir("notices-none")]).unwrap_err();
    assert_eq!(none.code, ErrorCode::NotFound);
}

// ---------------------------------------------------------------------------------------
// H14 — the first-run window fits the screen
// ---------------------------------------------------------------------------------------

#[test]
fn a_window_larger_than_the_work_area_is_shrunk_to_ninety_percent_and_centred() {
    use seepdf_lib::fit_to_work_area;
    // 1400×900 logical at 125 % = 1750×1125 physical, on a 1366×768 panel at 100 % minus a
    // 40 px taskbar — and on a 1920×1080 panel at 150 % (1920×1032 physical work area).
    for (size, pos, work) in [
        ((1750u32, 1125u32), (0i32, 0i32), (1366u32, 728u32)),
        ((2100, 1350), (0, 0), (1920, 1032)),
        ((1400, 900), (1920, 0), (1280, 760)), // a second monitor to the right
    ] {
        let ((x, y), (w, h)) = fit_to_work_area(size, pos, work).expect("too large → fitted");
        assert!(
            w <= work.0 * 9 / 10 + 1 && h <= work.1 * 9 / 10 + 1,
            "{w}×{h} in {work:?}"
        );
        assert!(w <= work.0 && h <= work.1);
        assert!(x >= pos.0 && y >= pos.1);
        assert!(x as i64 + w as i64 <= pos.0 as i64 + work.0 as i64);
        assert!(y as i64 + h as i64 <= pos.1 as i64 + work.1 as i64);
        // centred (within a pixel of rounding)
        let left = x - pos.0;
        let right = (pos.0 + work.0 as i32) - (x + w as i32);
        assert!((left - right).abs() <= 1, "{left} vs {right}");
    }
    // A window that fits is left exactly where the user (or window-state) put it.
    assert_eq!(fit_to_work_area((1400, 900), (0, 0), (2560, 1400)), None);
    // Only the dimension that overflows shrinks.
    let (_, (w, h)) = fit_to_work_area((1000, 900), (0, 0), (1920, 800)).unwrap();
    assert_eq!((w, h), (1000, 720));
}
