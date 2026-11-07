//! UI texts loaded from `locales/<lang>.json`.
//!
//! The JSON files are flat objects `{ "key": "text" }`; `{name}` in a text is
//! replaced by [`tf`]. Both files are embedded in the program. A file of the
//! same name in a `locales` folder next to the executable takes precedence,
//! so texts can be edited without rebuilding.
//!
//! The language is global: [`set_language`] switches every text at once and
//! remembers the choice in `%APPDATA%\FastFAT\language.txt`.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Embedded Japanese texts.
const JP_JSON: &str = include_str!("../../locales/jp.json");

/// Embedded English texts.
const EN_JSON: &str = include_str!("../../locales/en.json");

/// A UI language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Language {
    /// Japanese (`jp.json`).
    Japanese,
    /// English (`en.json`).
    English,
}

impl Language {
    /// Every language, in the order shown.
    pub(super) const ALL: [Self; 2] = [Self::Japanese, Self::English];

    /// Returns the name of the language in itself.
    pub(super) const fn native_name(self) -> &'static str {
        match self {
            Self::Japanese => "日本語",
            Self::English => "English",
        }
    }

    /// Returns the file stem of the language's JSON file.
    const fn code(self) -> &'static str {
        match self {
            Self::Japanese => "jp",
            Self::English => "en",
        }
    }

    /// Returns the language with this code.
    fn from_code(code: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.code() == code.trim())
    }

    /// Returns the embedded JSON of the language.
    const fn embedded(self) -> &'static str {
        match self {
            Self::Japanese => JP_JSON,
            Self::English => EN_JSON,
        }
    }
}

/// Texts of one language.
type Texts = HashMap<&'static str, &'static str>;

/// Current language (index into [`Language::ALL`]).
static CURRENT: AtomicU8 = AtomicU8::new(0);

/// Loaded texts per language.
static TEXTS: [OnceLock<Texts>; 2] = [OnceLock::new(), OnceLock::new()];

/// Returns the current language.
pub(super) fn language() -> Language {
    Language::ALL[usize::from(CURRENT.load(Ordering::Relaxed)) % Language::ALL.len()]
}

/// Switches the language and remembers the choice.
pub(super) fn set_language(language: Language) {
    store_index(language);
    if let Some(path) = settings_path() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, language.code());
    }
}

/// Switches the language without remembering it.
#[cfg(test)]
pub(super) fn set_language_for_test(language: Language) {
    store_index(language);
}

/// Restores the language chosen last time (Japanese by default).
pub(super) fn load_language() {
    let saved = settings_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|code| Language::from_code(&code));
    store_index(saved.unwrap_or(Language::Japanese));
}

/// Stores `language` as the current one.
fn store_index(language: Language) {
    let index = Language::ALL
        .iter()
        .position(|&l| l == language)
        .unwrap_or(0);
    CURRENT.store(u8::try_from(index).unwrap_or(0), Ordering::Relaxed);
}

/// Returns `%APPDATA%\FastFAT\language.txt`.
fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("FastFAT").join("language.txt"))
}

/// Returns the texts of `language`, loading them on first use.
fn texts(language: Language) -> &'static Texts {
    let index = Language::ALL
        .iter()
        .position(|&l| l == language)
        .unwrap_or(0);
    TEXTS[index].get_or_init(|| {
        let external = std::env::current_exe()
            .ok()
            .and_then(|exe| {
                exe.parent().map(|dir| {
                    dir.join("locales")
                        .join(format!("{}.json", language.code()))
                })
            })
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|json| parse(&json));
        external
            .or_else(|| parse(language.embedded()))
            .unwrap_or_default()
    })
}

/// Parses a flat JSON object of strings; the texts live for the rest of the
/// program.
fn parse(json: &str) -> Option<Texts> {
    let map: HashMap<String, String> = serde_json::from_str(json).ok()?;
    Some(
        map.into_iter()
            .map(|(k, v)| {
                (
                    &*Box::leak(k.into_boxed_str()),
                    &*Box::leak(v.into_boxed_str()),
                )
            })
            .collect(),
    )
}

/// Returns the text of `key` in the current language, falling back to
/// English and then to the key itself.
pub(super) fn t(key: &'static str) -> &'static str {
    texts(language())
        .get(key)
        .or_else(|| texts(Language::English).get(key))
        .copied()
        .unwrap_or(key)
}

/// Returns the text of `key` with every `{name}` replaced by its value.
pub(super) fn tf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    let mut text = t(key).to_owned();
    for (name, value) in args {
        text = text.replace(&format!("{{{name}}}"), &value.to_string());
    }
    text
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use super::{EN_JSON, JP_JSON};

    fn load(json: &str) -> HashMap<String, String> {
        serde_json::from_str(json).unwrap_or_else(|e| panic!("invalid JSON: {e}"))
    }

    fn placeholders(text: &str) -> BTreeSet<String> {
        text.split('{')
            .skip(1)
            .filter_map(|rest| rest.split_once('}').map(|(name, _)| name.to_owned()))
            .collect()
    }

    #[test]
    fn languages_have_the_same_keys_and_placeholders() {
        let jp = load(JP_JSON);
        let en = load(EN_JSON);
        let jp_keys: BTreeSet<_> = jp.keys().collect();
        let en_keys: BTreeSet<_> = en.keys().collect();
        assert_eq!(jp_keys, en_keys);
        for (key, text) in &jp {
            assert_eq!(
                placeholders(text),
                placeholders(&en[key]),
                "placeholders of {key}"
            );
        }
    }

    #[test]
    fn every_key_used_in_the_sources_exists() {
        let en = load(EN_JSON);
        let prefixes = [
            "app.",
            "tab.",
            "pane.",
            "common.",
            "clone.",
            "dialog.",
            "format.",
            "ftp.",
            "job.",
            "transfer.",
        ];
        let mut checked = 0;
        let mut folders = vec![
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src")
                .join("app"),
        ];
        while let Some(folder) = folders.pop() {
            for file in std::fs::read_dir(folder).unwrap_or_else(|e| panic!("{e}")) {
                let path = file.unwrap_or_else(|e| panic!("{e}")).path();
                if path.is_dir() {
                    folders.push(path);
                    continue;
                }
                if path.file_name().is_some_and(|name| name == "i18n.rs") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap_or_default();
                for literal in source.split('"').skip(1).step_by(2) {
                    let is_key = prefixes.iter().any(|p| literal.starts_with(p))
                        && literal.chars().all(|c| {
                            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '.' || c == '_'
                        });
                    if is_key {
                        checked += 1;
                        assert!(
                            en.contains_key(literal),
                            "{}: missing key {literal}",
                            path.display()
                        );
                    }
                }
            }
        }
        // Guards against a scan that silently finds nothing.
        assert!(checked >= en.len(), "only {checked} key uses found");
    }
}
