//! v0.3 integration (X8): is SeePDF itself the system's default PDF application?
//!
//! 인쇄 ▸ PDF 앱으로 인쇄 hands a flattened temp copy to the default `.pdf` handler. When that
//! handler is SeePDF, the copy would come straight back — through the single-instance hand-off
//! it opens as a document (and is swept away 10 minutes later while still open) instead of
//! being printed. The print flow asks first and prints through the webview then.
//!
//! Windows: `AssocQueryStringW(ASSOCSTR_EXECUTABLE, ".pdf", "open")` against the running
//! executable. Elsewhere: `false` (macOS prints through the webview; the handler route is a
//! Windows fallback).

use std::path::Path;

/// Whether `handler` and `own` name the same executable file.
pub fn same_executable(handler: &Path, own: &Path) -> bool {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let (a, b) = (canonical(handler), canonical(own));
    a == b
        || a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase() && cfg!(windows)
}

/// `pdf_handler_is_self`.
pub fn pdf_handler_is_self() -> bool {
    match (default_pdf_handler(), std::env::current_exe()) {
        (Some(handler), Ok(own)) => same_executable(Path::new(&handler), &own),
        _ => false,
    }
}

#[cfg(windows)]
fn default_pdf_handler() -> Option<String> {
    use windows::core::{w, PWSTR};
    use windows::Win32::UI::Shell::{
        AssocQueryStringW, ASSOCF_INIT_IGNOREUNKNOWN, ASSOCF_NOTRUNCATE, ASSOCSTR_EXECUTABLE,
    };
    let flags = ASSOCF_INIT_IGNOREUNKNOWN | ASSOCF_NOTRUNCATE;
    let mut len: u32 = 0;
    // The first call only measures (`S_FALSE` and the length with the terminator).
    let _ = unsafe {
        AssocQueryStringW(
            flags,
            ASSOCSTR_EXECUTABLE,
            w!(".pdf"),
            w!("open"),
            None,
            &mut len,
        )
    };
    if len == 0 {
        return None;
    }
    let mut buf = vec![0u16; len as usize];
    let hr = unsafe {
        AssocQueryStringW(
            flags,
            ASSOCSTR_EXECUTABLE,
            w!(".pdf"),
            w!("open"),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
    };
    if hr.is_err() {
        return None;
    }
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Some(String::from_utf16_lossy(&buf[..end]))
}

#[cfg(not(windows))]
fn default_pdf_handler() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_running_executable_is_itself() {
        let own = std::env::current_exe().unwrap();
        assert!(same_executable(&own, &own));
        assert!(!same_executable(Path::new("/nowhere/AcroRd32.exe"), &own));
    }

    #[test]
    fn not_the_handler_off_windows() {
        if !cfg!(windows) {
            assert!(!pdf_handler_is_self());
        }
    }
}
