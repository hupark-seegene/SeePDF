//! The step names of the native 편집 menu's 실행 취소 / 다시 실행 items (Stage 8).
//!
//! One source of truth: the strings are the frontend's own `src/i18n/{ko,en}.json`, compiled in
//! with `include_str!` and parsed once. An undo label is an i18n key (`"undo.watermark"`, what
//! `DocInfo.undoLabel` carries); the item text is the frontend's `menu.edit.undoAction`
//! template with that step's name (`실행 취소: 워터마크` / `Undo Watermark`), or the plain
//! `menu.edit.undo` when there is nothing to undo or the key is unknown.
//!
//! Pure and platform-independent so it is unit-tested everywhere; `app::menu` (macOS) applies
//! the result to the menu bar.

use crate::ipc::types::Locale;
use std::collections::HashMap;
use std::sync::OnceLock;

const KO_JSON: &str = include_str!("../../../src/i18n/ko.json");
const EN_JSON: &str = include_str!("../../../src/i18n/en.json");

/// Which of the two history items.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryItem {
    Undo,
    Redo,
}

fn table(locale: Locale) -> &'static HashMap<String, String> {
    static KO: OnceLock<HashMap<String, String>> = OnceLock::new();
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    let (cell, json) = match locale {
        Locale::Ko => (&KO, KO_JSON),
        Locale::En => (&EN, EN_JSON),
    };
    cell.get_or_init(|| parse(json))
}

/// A flat `{ "key": "string" }` object; anything else (a nested table, a non-string value) is
/// skipped rather than failing — a menu label is never worth a panic.
fn parse(json: &str) -> HashMap<String, String> {
    match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(json) {
        Ok(map) => map
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k, s.to_string())))
            .collect(),
        Err(e) => {
            tracing::warn!("i18n table did not parse: {e}");
            HashMap::new()
        }
    }
}

/// `t(key)` in `locale`, if the table has it.
pub fn text(key: &str, locale: Locale) -> Option<&'static str> {
    table(locale).get(key).map(String::as_str)
}

/// The step name of an undo label key (`"undo.watermark"` → `워터마크`).
pub fn step_name(key: &str, locale: Locale) -> Option<&'static str> {
    if !key.starts_with("undo.") {
        return None;
    }
    text(key, locale)
}

/// The item text for `item` when the history's top entry is `key`.
pub fn item_label(item: HistoryItem, key: Option<&str>, locale: Locale) -> String {
    let (plain_key, template_key, fallback) = match (item, locale) {
        (HistoryItem::Undo, Locale::Ko) => ("menu.edit.undo", "menu.edit.undoAction", "실행 취소"),
        (HistoryItem::Undo, Locale::En) => ("menu.edit.undo", "menu.edit.undoAction", "Undo"),
        (HistoryItem::Redo, Locale::Ko) => ("menu.edit.redo", "menu.edit.redoAction", "다시 실행"),
        (HistoryItem::Redo, Locale::En) => ("menu.edit.redo", "menu.edit.redoAction", "Redo"),
    };
    let plain = text(plain_key, locale).unwrap_or(fallback);
    let Some(step) = key.and_then(|k| step_name(k, locale)) else {
        return plain.to_string();
    };
    match text(template_key, locale) {
        Some(template) if template.contains("{{action}}") => template.replace("{{action}}", step),
        _ => format!("{plain} {step}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_undo_label_has_a_step_name_in_both_languages() {
        // Every label a `MutateOpts::new(..)` in the engine uses.
        let keys = [
            "undo.annotCreate",
            "undo.annotDelete",
            "undo.annotEdit",
            "undo.formFill",
            "undo.formReset",
            "undo.metadataEdit",
            "undo.metadataRemove",
            "undo.objectAdd",
            "undo.objectDelete",
            "undo.objectEdit",
            "undo.paragraphEdit",
            "undo.objectTransform",
            "undo.objectDuplicate",
            "undo.ocrApply",
            "undo.redact",
            "undo.watermark",
            "undo.headerFooter",
            "undo.removeStamps",
            "undo.compress",
            "undo.pageOps",
            "undo.pageMove",
            "undo.pageDelete",
            "undo.pageRotate",
            "undo.pageInsert",
            "undo.pageDuplicate",
            "undo.pageInsertFrom",
            "undo.pageReverse",
        ];
        for key in keys {
            for locale in [Locale::Ko, Locale::En] {
                assert!(
                    step_name(key, locale).is_some(),
                    "{key} missing in {locale:?}"
                );
            }
        }
    }

    #[test]
    fn menu_label_mapping() {
        assert_eq!(
            item_label(HistoryItem::Undo, Some("undo.watermark"), Locale::En),
            "Undo Watermark"
        );
        assert_eq!(
            item_label(HistoryItem::Redo, Some("undo.watermark"), Locale::En),
            "Redo Watermark"
        );
        // Korean follows the frontend's own template (`menu.edit.undoAction`), whatever its
        // punctuation: "실행 취소: 워터마크" today.
        let ko = item_label(HistoryItem::Undo, Some("undo.watermark"), Locale::Ko);
        assert!(
            ko.starts_with("실행 취소") && ko.ends_with("워터마크"),
            "{ko}"
        );
        let template = text("menu.edit.undoAction", Locale::Ko).expect("the frontend template");
        assert_eq!(ko, template.replace("{{action}}", "워터마크"));
        let ko = item_label(HistoryItem::Undo, Some("undo.removeStamps"), Locale::Ko);
        assert!(
            ko.starts_with("실행 취소") && ko.ends_with("워터마크 제거"),
            "{ko}"
        );
        let ko = item_label(HistoryItem::Redo, Some("undo.redact"), Locale::Ko);
        assert!(
            ko.starts_with("다시 실행") && ko.ends_with("영역 삭제"),
            "{ko}"
        );
        assert_eq!(
            item_label(HistoryItem::Redo, Some("undo.objectDuplicate"), Locale::En),
            "Redo Duplicate Object"
        );
        // Nothing to undo, an unknown key, a key outside `undo.*`: the plain item text.
        assert_eq!(item_label(HistoryItem::Undo, None, Locale::Ko), "실행 취소");
        assert_eq!(item_label(HistoryItem::Redo, None, Locale::En), "Redo");
        assert_eq!(
            item_label(HistoryItem::Undo, Some("undo.nope"), Locale::En),
            "Undo"
        );
        assert_eq!(
            item_label(HistoryItem::Undo, Some("menu.file"), Locale::Ko),
            "실행 취소"
        );
    }
}
