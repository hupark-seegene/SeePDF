//! Diagnostics (v0.3 pkg5, H10): the rolling log file, the panic hook, the "cannot start"
//! message box and the 문제 보고 text.
//!
//! * **Logs.** Release builds used to log to stderr only, which a `windows_subsystem =
//!   "windows"` process does not even have. [`init_tracing`] adds a daily rolling file in the
//!   app log directory (`~/Library/Logs/com.seepdf.desktop/` on macOS,
//!   `%LOCALAPPDATA%\com.seepdf.desktop\logs\` on Windows — the directory tauri's
//!   `app_log_dir` names), `seepdf.YYYY-MM-DD.log`, the newest [`LOG_FILES_KEPT`] kept. The
//!   writer is synchronous: a line is on disk before the process can exit or abort.
//! * **Panics** are logged by [`install_panic_hook`] (thread, location, message) before the
//!   default hook prints them; the engine thread survives its own (`engine::thread`).
//! * **Startup failures** — pdfium missing or quarantined by an antivirus, the engine thread
//!   not starting — used to make the app vanish without a word. `lib.rs` now shows
//!   [`startup_failure_text`] in a native message box ([`show_fatal`]) and exits non-zero.
//! * **문제 보고…** ([`problem_report`]): version, OS / arch, pdfium version and the last
//!   [`REPORT_LOG_LINES`] log lines, as one text the frontend copies to the clipboard; the same
//!   text is written to `problem-report.txt` in the log folder, which the frontend reveals.

use crate::ipc::types::{AppInfo, Locale};
use std::path::{Path, PathBuf};
use tracing_appender::rolling::{RollingFileAppender, Rotation};

/// `identifier` in `tauri.conf.json` (a unit test keeps the two equal): the log directory is
/// needed before the Tauri app — and its path resolver — exists.
pub const APP_IDENTIFIER: &str = "com.seepdf.desktop";
/// `seepdf.2026-09-29.log`.
pub const LOG_PREFIX: &str = "seepdf";
pub const LOG_SUFFIX: &str = "log";
/// One file a day, a week of them.
pub const LOG_FILES_KEPT: usize = 7;
/// How much of the log 문제 보고 copies.
pub const REPORT_LOG_LINES: usize = 200;
/// Written next to the logs by 문제 보고, so the text survives a clipboard that refused it.
pub const REPORT_FILE: &str = "problem-report.txt";

/// The app log directory, resolved exactly as tauri's `PathResolver::app_log_dir` does.
pub fn log_dir() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    let dir = dirs::home_dir().map(|home| home.join("Library/Logs").join(APP_IDENTIFIER));
    #[cfg(not(target_os = "macos"))]
    let dir = dirs::data_local_dir().map(|local| local.join(APP_IDENTIFIER).join("logs"));
    dir
}

/// The daily rolling writer under `dir` (created if needed), keeping [`LOG_FILES_KEPT`] files.
pub fn file_appender(dir: &Path) -> Result<RollingFileAppender, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix(LOG_SUFFIX)
        .max_log_files(LOG_FILES_KEPT)
        .build(dir)
        .map_err(|e| format!("log file in {}: {e}", dir.display()))
}

/// stderr (as before) plus, when `dir` is usable, the rolling file. Idempotent: a second call
/// (a test harness that already installed a subscriber) is a no-op.
pub fn init_tracing(dir: Option<&Path>) {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::{fmt, EnvFilter};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,seepdf_lib=debug,webview=info"));
    let file = dir.and_then(|d| match file_appender(d) {
        Ok(writer) => Some(writer),
        Err(e) => {
            eprintln!("SeePDF: no log file: {e}");
            None
        }
    });
    let file_layer = file.map(|writer| {
        fmt::layer()
            .with_ansi(false)
            .with_thread_names(true)
            .with_target(true)
            .with_writer(writer)
    });
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_thread_names(true)
                .with_target(true)
                .with_writer(std::io::stderr),
        )
        .with(file_layer)
        .try_init();
}

/// A panic's payload as text (`panic!("…")` gives a `&str` or a `String`).
pub fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Logs every panic (thread, location, message) and then runs the previous hook, so the
/// default stderr output is unchanged.
pub fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let location = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        tracing::error!(
            target: "seepdf_lib::panic",
            thread = thread.name().unwrap_or("?"),
            %location,
            "panic: {}",
            panic_message(info.payload())
        );
        previous(info);
    }));
}

/// Why the app could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupFailure {
    /// libpdfium is missing, quarantined or cannot be bound — reinstall / antivirus exception.
    Library(String),
    /// Anything else (the webview, a window, the menu).
    Other(String),
}

/// `(title, body)` of the startup-failure message box, in the app language.
pub fn startup_failure_text(
    failure: &StartupFailure,
    locale: Locale,
    log_dir: Option<&Path>,
) -> (String, String) {
    let ko = locale == Locale::Ko;
    let title = if ko {
        "SeePDF를 시작할 수 없습니다"
    } else {
        "SeePDF cannot start"
    };
    let (headline, detail) = match failure {
        StartupFailure::Library(detail) => (
            if ko {
                "PDF 라이브러리를 불러올 수 없습니다 — 재설치하거나 백신 예외를 추가하세요."
            } else {
                "The PDF library could not be loaded — reinstall SeePDF or add an antivirus exception."
            },
            detail,
        ),
        StartupFailure::Other(detail) => (
            if ko {
                "SeePDF를 시작하는 중 문제가 발생했습니다. 다시 설치해 보세요."
            } else {
                "Something went wrong while starting SeePDF. Try reinstalling it."
            },
            detail,
        ),
    };
    let mut body = format!(
        "{headline}\n\n{}: {detail}",
        if ko { "오류" } else { "Error" }
    );
    if let Some(dir) = log_dir {
        body.push_str(&format!(
            "\n{}: {}",
            if ko { "로그" } else { "Log" },
            dir.display()
        ));
    }
    (title.to_string(), body)
}

/// A blocking native message box. Called on the main thread from `setup`, before any webview.
pub fn show_fatal(title: &str, body: &str) {
    #[cfg(any(target_os = "macos", windows))]
    {
        let _ = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title(title)
            .set_description(body)
            .set_buttons(rfd::MessageButtons::Ok)
            .show();
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    eprintln!("{title}\n{body}");
}

/// Logs the failure, shows the message box and ends the process with exit code 1.
pub fn fail_startup(failure: StartupFailure, locale: Locale) -> ! {
    tracing::error!(?failure, "startup failed");
    let (title, body) = startup_failure_text(&failure, locale, log_dir().as_deref());
    show_fatal(&title, &body);
    std::process::exit(1);
}

/// The log files in `dir`, oldest first (the date in the name sorts).
fn log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .map(|n| n.starts_with(LOG_PREFIX) && n.ends_with(LOG_SUFFIX))
                        .unwrap_or(false)
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
}

/// The last `n` lines across the newest log files (a report just after midnight still gets
/// yesterday's tail).
pub fn recent_log_lines(dir: &Path, n: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for file in log_files(dir).iter().rev() {
        if out.len() >= n {
            break;
        }
        let Ok(bytes) = std::fs::read(file) else {
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let lines: Vec<&str> = text.lines().collect();
        let take = (n - out.len()).min(lines.len());
        let mut chunk: Vec<String> = lines[lines.len() - take..]
            .iter()
            .map(|l| l.to_string())
            .collect();
        chunk.append(&mut out);
        out = chunk;
    }
    out
}

/// The 문제 보고 text.
pub fn problem_report(info: &AppInfo, log_lines: &[String]) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "SeePDF {}{}\n",
        info.version,
        if info.debug { " (debug)" } else { "" }
    ));
    s.push_str(&format!("OS: {} / {}\n", info.os, info.arch));
    s.push_str(&format!("PDFium: {}\n", info.pdfium_version));
    s.push_str(&format!("Locale: {:?}\n", info.locale).to_lowercase());
    s.push_str(&format!("Log lines ({}):\n", log_lines.len()));
    for line in log_lines {
        s.push_str(line);
        s.push('\n');
    }
    s
}

/// Writes `text` to [`REPORT_FILE`] in `dir` and returns its path.
pub fn write_report(dir: &Path, text: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    let path = dir.join(REPORT_FILE);
    std::fs::write(&path, text).map_err(|e| format!("write {}: {e}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifier_matches_tauri_conf() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], APP_IDENTIFIER);
    }

    #[test]
    fn panic_message_reads_both_payload_kinds() {
        let a: Box<dyn std::any::Any + Send> = Box::new("boom");
        let b: Box<dyn std::any::Any + Send> = Box::new(String::from("bang"));
        assert_eq!(panic_message(a.as_ref()), "boom");
        assert_eq!(panic_message(b.as_ref()), "bang");
    }
}
