//! The native macOS menu bar.
//!
//! Item ids are the `UI_SPEC.md` §15.2 keys with the `menu.` prefix stripped
//! (`menu.file.open` → id `file.open`), which is also how the frontend keymap names its
//! commands. Selecting an item emits `menu:<id>` to the focused window
//! (`IPC_CONTRACT.md` §8); the frontend routes it through the same table as the keyboard
//! shortcut, so there is exactly one implementation per command.
//!
//! **Labels follow the app language.** Korean is the default (UI_SPEC §15.2), English when
//! `Settings.locale` is `en`. macOS caches the menu, so changing the setting cannot re-label
//! the existing items — [`rebuild`] builds a whole new [`Menu`] and calls `set_menu` on the
//! main thread. `commands::app::set_settings` calls it when the locale actually changed, so
//! 설정 › 일반 relabels the menu bar without a restart.
//!
//! The strings live in [`LABELS`] rather than in `src/i18n/*.json`: the menu is built before
//! the webview exists, and a Rust-side copy of two dozen labels is cheaper than a round trip
//! through the frontend. `menu_labels_cover_every_item` asserts the table is complete.

#![cfg(target_os = "macos")]

use crate::ipc::types::Locale;
use tauri::menu::{AboutMetadata, Menu, MenuEvent, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{AppHandle, Emitter, Manager, Runtime, Wry};

/// Every id this menu can emit. Kept public so a test (and the frontend's keymap) can assert
/// the two tables agree.
pub const MENU_IDS: &[&str] = &[
    "file.open",
    "file.openRecent",
    "file.clearRecent",
    "file.close",
    "file.save",
    "file.saveAs",
    "file.export",
    "file.print",
    "file.docInfo",
    "file.revealInFinder",
    "edit.undo",
    "edit.redo",
    "edit.delete",
    "edit.duplicate",
    "edit.deselect",
    "edit.selectAll",
    "edit.find",
    "edit.findNext",
    "edit.findPrevious",
    "view.sidebar",
    "view.inspector",
    "view.zoomIn",
    "view.zoomOut",
    "view.actualSize",
    "view.fitPage",
    "view.fitWidth",
    "view.readingMode",
    "go.nextPage",
    "go.previousPage",
    "go.firstPage",
    "go.lastPage",
    "go.goToPage",
    "go.back",
    "go.forward",
    "tools.ocr",
    "tools.security",
    "tools.redact",
    "tools.stamp",
    "tools.compress",
    "tools.merge",
    "help.shortcuts",
    "settings",
];

/// `(key, 한국어, English)` — the `menu.*` rows of `UI_SPEC.md` §15.2, verbatim. The key is
/// the `menu.`-stripped id for a command item, and the submenu / predefined-item name for the
/// rest (`file`, `edit`, `quit`, `window.minimize`, …).
const LABELS: &[(&str, &str, &str)] = &[
    // submenu titles
    ("file", "파일", "File"),
    ("edit", "편집", "Edit"),
    ("view", "보기", "View"),
    ("go", "이동", "Go"),
    ("tools", "도구", "Tools"),
    ("window", "윈도우", "Window"),
    ("help", "도움말", "Help"),
    // app menu
    ("settings", "설정…", "Settings…"),
    ("help.about", "SeePDF 정보", "About SeePDF"),
    ("quit", "SeePDF 종료", "Quit SeePDF"),
    ("services", "서비스", "Services"),
    ("hide", "SeePDF 가리기", "Hide SeePDF"),
    ("hideOthers", "기타 가리기", "Hide Others"),
    ("showAll", "모두 보기", "Show All"),
    // file
    ("file.open", "열기…", "Open…"),
    ("file.openRecent", "최근 항목 열기", "Open Recent"),
    ("file.clearRecent", "메뉴 지우기", "Clear Menu"),
    ("file.close", "닫기", "Close"),
    ("file.save", "저장", "Save"),
    ("file.saveAs", "다른 이름으로 저장…", "Save As…"),
    ("file.export", "내보내기…", "Export…"),
    ("file.print", "인쇄…", "Print…"),
    ("file.docInfo", "문서 정보", "Document Properties"),
    ("file.revealInFinder", "Finder에서 보기", "Reveal in Finder"),
    // edit
    ("edit.undo", "실행 취소", "Undo"),
    ("edit.redo", "다시 실행", "Redo"),
    ("edit.cut", "잘라내기", "Cut"),
    ("edit.copy", "복사", "Copy"),
    ("edit.paste", "붙여넣기", "Paste"),
    ("edit.delete", "삭제", "Delete"),
    ("edit.duplicate", "복제", "Duplicate"),
    ("edit.selectAll", "전체 선택", "Select All"),
    ("edit.deselect", "선택 해제", "Deselect"),
    ("edit.find", "찾기…", "Find…"),
    ("edit.findNext", "다음 찾기", "Find Next"),
    ("edit.findPrevious", "이전 찾기", "Find Previous"),
    // view
    ("view.sidebar", "사이드바", "Sidebar"),
    ("view.inspector", "속성 패널", "Inspector"),
    ("view.zoomIn", "확대", "Zoom In"),
    ("view.zoomOut", "축소", "Zoom Out"),
    ("view.actualSize", "실제 크기", "Actual Size"),
    ("view.fitPage", "페이지에 맞춤", "Fit Page"),
    ("view.fitWidth", "너비에 맞춤", "Fit Width"),
    ("view.readingMode", "읽기 모드", "Reading Mode"),
    ("view.fullScreen", "전체 화면", "Full Screen"),
    // go
    ("go.nextPage", "다음 페이지", "Next Page"),
    ("go.previousPage", "이전 페이지", "Previous Page"),
    ("go.firstPage", "첫 페이지", "First Page"),
    ("go.lastPage", "마지막 페이지", "Last Page"),
    ("go.goToPage", "페이지로 이동…", "Go to Page…"),
    ("go.back", "뒤로", "Back"),
    ("go.forward", "앞으로", "Forward"),
    // tools
    ("tools.ocr", "텍스트 인식(OCR)…", "Recognize Text (OCR)…"),
    ("tools.security", "보안…", "Security…"),
    ("tools.redact", "영역 표시", "Redact"),
    ("tools.stamp", "워터마크 / 머리글·바닥글…", "Watermark / Header & Footer…"),
    ("tools.compress", "압축…", "Compress…"),
    ("tools.merge", "파일 합치기…", "Merge Files…"),
    // window / help
    ("window.minimize", "최소화", "Minimize"),
    ("window.zoom", "확대/축소", "Zoom"),
    ("window.close", "닫기", "Close"),
    ("help.shortcuts", "단축키", "Keyboard Shortcuts"),
];

/// The label of one menu key in the app language. Panics only in debug builds; a release
/// build falls back to the key so a missing row can never take the menu bar down.
fn label(key: &str, locale: Locale) -> &'static str {
    match LABELS.iter().find(|(k, _, _)| *k == key) {
        Some((_, ko, en)) => {
            if locale == Locale::Ko {
                ko
            } else {
                en
            }
        }
        None => {
            debug_assert!(false, "menu label {key} is missing from LABELS");
            ""
        }
    }
}

pub fn build<R: Runtime>(app: &AppHandle<R>, locale: Locale) -> tauri::Result<Menu<R>> {
    // `t` is the label of a key; `item` is a custom item whose id *is* its label key.
    let t = |key: &str| label(key, locale);
    let item = |id: &str, accel: Option<&str>| -> tauri::Result<_> {
        let mut builder = MenuItemBuilder::with_id(id.to_string(), label(id, locale));
        if let Some(accel) = accel {
            builder = builder.accelerator(accel);
        }
        builder.build(app)
    };

    let app_menu = SubmenuBuilder::new(app, "SeePDF")
        .item(&PredefinedMenuItem::about(
            app,
            Some(t("help.about")),
            Some(AboutMetadata::default()),
        )?)
        .separator()
        .item(&item("settings", Some("CmdOrCtrl+,"))?)
        .separator()
        .item(&PredefinedMenuItem::services(app, Some(t("services")))?)
        .separator()
        .item(&PredefinedMenuItem::hide(app, Some(t("hide")))?)
        .item(&PredefinedMenuItem::hide_others(app, Some(t("hideOthers")))?)
        .item(&PredefinedMenuItem::show_all(app, Some(t("showAll")))?)
        .separator()
        .item(&PredefinedMenuItem::quit(app, Some(t("quit")))?)
        .build()?;

    let file_menu = SubmenuBuilder::new(app, t("file"))
        .item(&item("file.open", Some("CmdOrCtrl+O"))?)
        .item(&item("file.openRecent", Some("CmdOrCtrl+Shift+O"))?)
        .item(&item("file.clearRecent", None)?)
        .separator()
        .item(&item("file.close", Some("CmdOrCtrl+W"))?)
        .item(&item("file.save", Some("CmdOrCtrl+S"))?)
        .item(&item("file.saveAs", Some("CmdOrCtrl+Shift+S"))?)
        .item(&item("file.export", Some("CmdOrCtrl+Alt+E"))?)
        .separator()
        .item(&item("file.print", Some("CmdOrCtrl+P"))?)
        .separator()
        .item(&item("file.docInfo", Some("CmdOrCtrl+I"))?)
        .item(&item("file.revealInFinder", None)?)
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, t("edit"))
        .item(&item("edit.undo", Some("CmdOrCtrl+Z"))?)
        .item(&item("edit.redo", Some("CmdOrCtrl+Shift+Z"))?)
        .separator()
        // Cut / Copy / Paste stay **predefined**: macOS routes them through the responder
        // chain into WKWebView, which is what makes ⌘C work in a text field, in a form widget
        // and over the canvas (the clipboard mirror of `viewerCommands.ts`). A custom item
        // would emit an event instead, and `document.execCommand("copy")` from a Tauri event
        // callback has no user activation — F-06 would regress. The annotation clipboard is
        // reached from the DOM `copy`/`cut`/`paste` events these items raise
        // (`AnnotationHost.tsx`), which is the same seam with no such cost. They still carry
        // our own labels, so the menu bar is Korean throughout.
        .item(&PredefinedMenuItem::cut(app, Some(t("edit.cut")))?)
        .item(&PredefinedMenuItem::copy(app, Some(t("edit.copy")))?)
        .item(&PredefinedMenuItem::paste(app, Some(t("edit.paste")))?)
        // Select All *is* custom: 페이지 mode selects cells, the canvas selects the page's
        // text, and an input selects its own value — none of which WebKit can do for us, and
        // all of which are reachable with no user activation.
        .item(&item("edit.selectAll", Some("CmdOrCtrl+A"))?)
        // No accelerator: Esc is `tool.none` in the keymap and the menu must not shadow it.
        .item(&item("edit.deselect", None)?)
        .separator()
        .item(&item("edit.delete", None)?)
        .item(&item("edit.duplicate", Some("CmdOrCtrl+D"))?)
        .separator()
        .item(&item("edit.find", Some("CmdOrCtrl+F"))?)
        .item(&item("edit.findNext", Some("CmdOrCtrl+G"))?)
        .item(&item("edit.findPrevious", Some("CmdOrCtrl+Shift+G"))?)
        .build()?;

    let view_menu = SubmenuBuilder::new(app, t("view"))
        .item(&item("view.sidebar", Some("Ctrl+Cmd+S"))?)
        .item(&item("view.inspector", Some("Alt+Cmd+P"))?)
        .separator()
        .item(&item("view.zoomIn", Some("CmdOrCtrl+Plus"))?)
        .item(&item("view.zoomOut", Some("CmdOrCtrl+-"))?)
        .item(&item("view.actualSize", Some("CmdOrCtrl+0"))?)
        .item(&item("view.fitPage", Some("CmdOrCtrl+9"))?)
        .item(&item("view.fitWidth", Some("CmdOrCtrl+8"))?)
        .separator()
        .item(&item("view.readingMode", Some("Ctrl+Cmd+R"))?)
        .item(&PredefinedMenuItem::fullscreen(
            app,
            Some(t("view.fullScreen")),
        )?)
        .build()?;

    let go_menu = SubmenuBuilder::new(app, t("go"))
        .item(&item("go.nextPage", None)?)
        .item(&item("go.previousPage", None)?)
        .separator()
        .item(&item("go.firstPage", Some("CmdOrCtrl+Up"))?)
        .item(&item("go.lastPage", Some("CmdOrCtrl+Down"))?)
        .item(&item("go.goToPage", Some("Alt+Cmd+G"))?)
        .separator()
        .item(&item("go.back", Some("CmdOrCtrl+["))?)
        .item(&item("go.forward", Some("CmdOrCtrl+]"))?)
        .build()?;

    let tools_menu = SubmenuBuilder::new(app, t("tools"))
        .item(&item("tools.ocr", None)?)
        .item(&item("tools.redact", Some("CmdOrCtrl+Shift+R"))?)
        .item(&item("tools.security", None)?)
        .item(&item("tools.stamp", Some("CmdOrCtrl+Alt+W"))?)
        .item(&item("tools.compress", None)?)
        .item(&item("tools.merge", None)?)
        .build()?;

    let window_menu = SubmenuBuilder::new(app, t("window"))
        .item(&PredefinedMenuItem::minimize(
            app,
            Some(t("window.minimize")),
        )?)
        .item(&PredefinedMenuItem::maximize(app, Some(t("window.zoom")))?)
        .separator()
        .item(&PredefinedMenuItem::close_window(
            app,
            Some(t("window.close")),
        )?)
        .build()?;

    let help_menu = SubmenuBuilder::new(app, t("help"))
        .item(&item("help.shortcuts", None)?)
        .build()?;

    Menu::with_items(
        app,
        &[
            &app_menu,
            &file_menu,
            &edit_menu,
            &view_menu,
            &go_menu,
            &tools_menu,
            &window_menu,
            &help_menu,
        ],
    )
}

/// Rebuild the menu bar in `locale` and install it.
///
/// macOS owns the `NSMenu`, so this must run on the main thread; `set_settings` is handled on
/// a Tauri command thread, hence the hop. Failures are logged, never propagated: a menu that
/// could not be relabelled is a cosmetic problem, not a reason to fail 설정 저장.
pub fn rebuild(app: &AppHandle<Wry>, locale: Locale) {
    let handle = app.clone();
    let result = app.run_on_main_thread(move || match build(&handle, locale) {
        Ok(menu) => {
            if let Err(e) = handle.set_menu(menu) {
                tracing::warn!("set_menu after a locale change failed: {e}");
            } else {
                tracing::info!(?locale, "menu bar rebuilt");
            }
        }
        Err(e) => tracing::warn!("rebuilding the menu failed: {e}"),
    });
    if let Err(e) = result {
        tracing::warn!("run_on_main_thread for the menu failed: {e}");
    }
}

/// Routes a menu selection to the focused window as `menu:<id>`.
pub fn on_menu_event(app: &AppHandle<Wry>, event: MenuEvent) {
    let id = event.id().0.clone();
    if !MENU_IDS.contains(&id.as_str()) {
        return; // A predefined item (Cut/Copy/Quit/…) the OS already handled.
    }
    // Tauri event names allow only [A-Za-z0-9-/:_]; keymap ids use dots (file.open) -> menu:file/open
    let topic = format!("menu:{}", id.replace('.', "/"));
    let focused = app
        .webview_windows()
        .into_iter()
        .find(|(_, w)| w.is_focused().unwrap_or(false))
        .map(|(label, _)| label);
    let result = match focused {
        Some(label) => app.emit_to(label, &topic, ()),
        None => app.emit(&topic, ()),
    };
    if let Err(e) = result {
        tracing::warn!("emit {topic} failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every id the menu can emit, and every submenu / predefined label `build` asks for, has
    /// a Korean **and** an English string. Without this the fallback would silently ship an
    /// empty menu title.
    #[test]
    fn menu_labels_cover_every_item() {
        let extra = [
            "file", "edit", "view", "go", "tools", "window", "help", "help.about", "quit",
            "services", "hide", "hideOthers", "showAll", "edit.cut", "edit.copy", "edit.paste",
            "view.fullScreen", "window.minimize", "window.zoom", "window.close",
        ];
        for key in MENU_IDS.iter().chain(extra.iter()) {
            let row = LABELS.iter().find(|(k, _, _)| k == key);
            let (_, ko, en) = row.unwrap_or_else(|| panic!("no LABELS row for {key}"));
            assert!(!ko.is_empty() && !en.is_empty(), "{key} has an empty label");
            assert_eq!(label(key, Locale::Ko), *ko);
            assert_eq!(label(key, Locale::En), *en);
        }
    }

    /// Korean is the default language (UI_SPEC §15.2), so the menu bar must be Korean out of
    /// the box and English only when the setting says so.
    #[test]
    fn korean_is_the_default_menu_language() {
        assert_eq!(label("file", Locale::Ko), "파일");
        assert_eq!(label("file", Locale::En), "File");
        assert_eq!(label("tools.ocr", Locale::Ko), "텍스트 인식(OCR)…");
        assert_eq!(
            label("file", crate::ipc::types::Settings::default().locale),
            "파일"
        );
    }
}
