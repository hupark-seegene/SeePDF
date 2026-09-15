//! The native macOS menu bar.
//!
//! Item ids are the `UI_SPEC.md` §15.2 keys with the `menu.` prefix stripped
//! (`menu.file.open` → id `file.open`), which is also how the frontend keymap names its
//! commands. Selecting an item emits `menu:<id>` to the focused window
//! (`IPC_CONTRACT.md` §8); the frontend routes it through the same table as the keyboard
//! shortcut, so there is exactly one implementation per command.
//!
//! Labels are English here: the menu bar is built before the webview reports its locale and
//! macOS caches the menu. Localising it is a P1 item (rebuild the menu on `set_settings`).

#![cfg(target_os = "macos")]

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
    "tools.compress",
    "tools.merge",
    "help.shortcuts",
    "settings",
];

pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let item = |id: &str, label: &str, accel: Option<&str>| -> tauri::Result<_> {
        let mut builder = MenuItemBuilder::with_id(id.to_string(), label);
        if let Some(accel) = accel {
            builder = builder.accelerator(accel);
        }
        builder.build(app)
    };

    let app_menu = SubmenuBuilder::new(app, "SeePDF")
        .item(&PredefinedMenuItem::about(
            app,
            Some("About SeePDF"),
            Some(AboutMetadata::default()),
        )?)
        .separator()
        .item(&item("settings", "Settings…", Some("CmdOrCtrl+,"))?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let file_menu = SubmenuBuilder::new(app, "File")
        .item(&item("file.open", "Open…", Some("CmdOrCtrl+O"))?)
        .item(&item(
            "file.openRecent",
            "Open Recent",
            Some("CmdOrCtrl+Shift+O"),
        )?)
        .item(&item("file.clearRecent", "Clear Menu", None)?)
        .separator()
        .item(&item("file.close", "Close", Some("CmdOrCtrl+W"))?)
        .item(&item("file.save", "Save", Some("CmdOrCtrl+S"))?)
        .item(&item("file.saveAs", "Save As…", Some("CmdOrCtrl+Shift+S"))?)
        .item(&item("file.export", "Export…", Some("CmdOrCtrl+Alt+E"))?)
        .separator()
        .item(&item("file.print", "Print…", Some("CmdOrCtrl+P"))?)
        .separator()
        .item(&item(
            "file.docInfo",
            "Document Properties",
            Some("CmdOrCtrl+I"),
        )?)
        .item(&item("file.revealInFinder", "Reveal in Finder", None)?)
        .build()?;

    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .item(&item("edit.undo", "Undo", Some("CmdOrCtrl+Z"))?)
        .item(&item("edit.redo", "Redo", Some("CmdOrCtrl+Shift+Z"))?)
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .item(&item("edit.deselect", "Deselect", None)?)
        .separator()
        .item(&item("edit.delete", "Delete", None)?)
        .item(&item("edit.duplicate", "Duplicate", Some("CmdOrCtrl+D"))?)
        .separator()
        .item(&item("edit.find", "Find…", Some("CmdOrCtrl+F"))?)
        .item(&item("edit.findNext", "Find Next", Some("CmdOrCtrl+G"))?)
        .item(&item(
            "edit.findPrevious",
            "Find Previous",
            Some("CmdOrCtrl+Shift+G"),
        )?)
        .build()?;

    let view_menu = SubmenuBuilder::new(app, "View")
        .item(&item("view.sidebar", "Sidebar", Some("Ctrl+Cmd+S"))?)
        .item(&item("view.inspector", "Inspector", Some("Alt+Cmd+P"))?)
        .separator()
        .item(&item("view.zoomIn", "Zoom In", Some("CmdOrCtrl+Plus"))?)
        .item(&item("view.zoomOut", "Zoom Out", Some("CmdOrCtrl+-"))?)
        .item(&item("view.actualSize", "Actual Size", Some("CmdOrCtrl+0"))?)
        .item(&item("view.fitPage", "Fit Page", Some("CmdOrCtrl+9"))?)
        .item(&item("view.fitWidth", "Fit Width", Some("CmdOrCtrl+8"))?)
        .separator()
        .item(&item("view.readingMode", "Reading Mode", Some("Ctrl+Cmd+R"))?)
        .item(&PredefinedMenuItem::fullscreen(app, Some("Full Screen"))?)
        .build()?;

    let go_menu = SubmenuBuilder::new(app, "Go")
        .item(&item("go.nextPage", "Next Page", None)?)
        .item(&item("go.previousPage", "Previous Page", None)?)
        .separator()
        .item(&item("go.firstPage", "First Page", Some("CmdOrCtrl+Up"))?)
        .item(&item("go.lastPage", "Last Page", Some("CmdOrCtrl+Down"))?)
        .item(&item("go.goToPage", "Go to Page…", Some("Alt+Cmd+G"))?)
        .separator()
        .item(&item("go.back", "Back", Some("CmdOrCtrl+["))?)
        .item(&item("go.forward", "Forward", Some("CmdOrCtrl+]"))?)
        .build()?;

    let tools_menu = SubmenuBuilder::new(app, "Tools")
        .item(&item("tools.ocr", "Recognize Text (OCR)…", None)?)
        .item(&item("tools.redact", "Redact", Some("CmdOrCtrl+Shift+R"))?)
        .item(&item("tools.security", "Security…", None)?)
        .item(&item("tools.compress", "Compress…", None)?)
        .item(&item("tools.merge", "Merge Files…", None)?)
        .build()?;

    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .close_window()
        .build()?;

    let help_menu = SubmenuBuilder::new(app, "Help")
        .item(&item("help.shortcuts", "Keyboard Shortcuts", None)?)
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
