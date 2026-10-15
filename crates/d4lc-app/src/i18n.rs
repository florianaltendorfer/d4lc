//! UI language: the Slint translation switch and the Rust-side string lookup.
//!
//! Upstream binds every caption through `TranslationSource`, which holds the
//! current culture, resolves a resource key against it and falls back to the
//! neutral English value (`D4Companion.Localization/TranslationSource.cs`).
//! Choosing a language assigns that culture, and every bound caption
//! re-evaluates (`SettingsViewModel.cs:222-235`).
//!
//! The port keeps the same shape. `translations/<culture>/LC_MESSAGES/
//! d4lc-app.po` are generated from the upstream resx files (`cargo xtask
//! import-upstream-locales`). Slint bundles them for `@tr` text and
//! [`select`] switches the bundle, which re-evaluates every `@tr` binding.
//! Strings Rust builds itself go through [`tr`] over the very same files, and
//! only where upstream has a key: the msgid is the upstream English value, so
//! a missing translation falls back to English exactly as upstream does.
//!
//! The selection is UI-thread state: Slint's translation context is
//! per-thread, and every caller of [`tr`] runs on the UI thread.

use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::LazyLock;

use d4lc_gui::settings::app_language_by_id;

/// The generated catalogues, by culture id.
const SOURCES: [(&str, &str); 5] = [
    (
        "de-DE",
        include_str!("../translations/de-DE/LC_MESSAGES/d4lc-app.po"),
    ),
    (
        "en-US",
        include_str!("../translations/en-US/LC_MESSAGES/d4lc-app.po"),
    ),
    (
        "es-ES",
        include_str!("../translations/es-ES/LC_MESSAGES/d4lc-app.po"),
    ),
    (
        "fr-FR",
        include_str!("../translations/fr-FR/LC_MESSAGES/d4lc-app.po"),
    ),
    (
        "zh-CN",
        include_str!("../translations/zh-CN/LC_MESSAGES/d4lc-app.po"),
    ),
];

/// The culture strings render in until a selection is applied.
const NEUTRAL_CULTURE: &str = "en-US";

static CATALOGUES: LazyLock<HashMap<&'static str, HashMap<String, String>>> = LazyLock::new(|| {
    SOURCES
        .iter()
        .map(|(culture, text)| (*culture, parse_po(text)))
        .collect()
});

thread_local! {
    static CURRENT: Cell<&'static str> = const { Cell::new(NEUTRAL_CULTURE) };
}

/// Why a language could not be applied.
#[derive(Debug)]
pub enum SelectError {
    /// The culture is not one the selector offers.
    UnknownCulture(String),
    /// Slint has no bundled translation to switch (no window exists on this
    /// thread yet, or the UI was built without the catalogues).
    NoBundle(String),
}

impl fmt::Display for SelectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCulture(culture) => write!(
                f,
                "unknown UI language '{culture}'; the selector offers only the upstream cultures"
            ),
            Self::NoBundle(detail) => write!(f, "cannot switch the UI language: {detail}"),
        }
    }
}

/// Switch the UI language: Slint's bundled `@tr` text and [`tr`].
///
/// Must run on the UI thread after the first window exists (Slint attaches
/// its bundle to the component context). The change re-evaluates every `@tr`
/// binding on its own; text Rust projected into properties has to be
/// projected again by the caller.
pub fn select(culture: &str) -> Result<(), SelectError> {
    let language = app_language_by_id(culture)
        .ok_or_else(|| SelectError::UnknownCulture(culture.to_string()))?;
    slint::select_bundled_translation(language.id)
        .map_err(|error| SelectError::NoBundle(error.to_string()))?;
    CURRENT.with(|current| current.set(language.id));
    Ok(())
}

/// The UI culture a desktop locale maps to, the way .NET resolves resources
/// for the OS UI culture: `Resources.de.resx` and `Resources.fr.resx` are
/// neutral-language satellites (any region), `Resources.es-ES.resx` and
/// `Resources.zh-CN.resx` are region-specific. `None` when no resource file
/// applies.
pub fn culture_for_locale(locale: &str) -> Option<&'static str> {
    let locale = locale.split(['.', '@']).next().unwrap_or_default();
    let mut parts = locale.split(['-', '_']);
    let language = parts.next()?.to_ascii_lowercase();
    let region = parts
        .filter(|part| part.len() == 2)
        .map(str::to_ascii_uppercase)
        .next();
    match (language.as_str(), region.as_deref()) {
        ("de", _) => Some("de-DE"),
        ("en", _) => Some("en-US"),
        ("fr", _) => Some("fr-FR"),
        ("es", Some("ES")) => Some("es-ES"),
        ("zh", Some("CN")) => Some("zh-CN"),
        _ => None,
    }
}

/// The culture to use for a stored `SelectedAppLanguage`.
///
/// A stored culture the selector offers is used as it is. Otherwise upstream
/// leaves its culture unset and WPF follows the OS UI culture
/// (`SettingsViewModel.cs:417-421`): the desktop locale decides if a resource
/// file applies, and English is the neutral fallback. The flag is set when the
/// stored value was not used, so the caller can warn.
pub fn resolve_culture(stored: &str, system_locale: Option<&str>) -> (&'static str, bool) {
    match app_language_by_id(stored) {
        Some(language) => (language.id, false),
        None => (
            system_locale
                .and_then(culture_for_locale)
                .unwrap_or(NEUTRAL_CULTURE),
            true,
        ),
    }
}

/// The culture [`tr`] currently resolves against on this thread.
pub fn current() -> &'static str {
    CURRENT.with(Cell::get)
}

/// The translation of an upstream English string in the current language.
///
/// Falls back to `msgid` itself when the culture has no translation for it,
/// like `TranslationSource` falls back to the neutral resource value.
pub fn tr(msgid: &str) -> String {
    lookup(current(), msgid).unwrap_or(msgid).to_string()
}

fn lookup(culture: &str, msgid: &str) -> Option<&'static str> {
    CATALOGUES
        .get(culture)?
        .get(msgid)
        .map(String::as_str)
        .filter(|translation| !translation.is_empty())
}

/// Parse a generated `.po` file into msgid -> msgstr. The header entry and
/// untranslated entries (empty msgstr) are left out.
fn parse_po(text: &str) -> HashMap<String, String> {
    #[derive(Clone, Copy, PartialEq)]
    enum Field {
        Id,
        Str,
    }
    let mut messages = HashMap::new();
    let (mut id, mut translation) = (String::new(), String::new());
    let mut field = None;
    // Entries with a message context are the port's own strings that collide
    // with an upstream value (`@tr("d4lc" => ...)`); Rust looks up upstream
    // strings only, so they never answer a lookup.
    let mut contextual = false;
    let mut flush = |id: &mut String, translation: &mut String, contextual: &mut bool| {
        if !id.is_empty() && !translation.is_empty() && !*contextual {
            messages.insert(std::mem::take(id), std::mem::take(translation));
        }
        id.clear();
        translation.clear();
        *contextual = false;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("msgctxt ") {
            flush(&mut id, &mut translation, &mut contextual);
            contextual = true;
            field = None;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if !contextual {
                flush(&mut id, &mut translation, &mut contextual);
            }
            id = unquote(rest);
            field = Some(Field::Id);
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            translation = unquote(rest);
            field = Some(Field::Str);
        } else if line.starts_with('"') {
            // A continuation line extends the field above it.
            match field {
                Some(Field::Id) => id.push_str(&unquote(line)),
                Some(Field::Str) => translation.push_str(&unquote(line)),
                None => {}
            }
        } else if line.is_empty() {
            flush(&mut id, &mut translation, &mut contextual);
            field = None;
        }
    }
    flush(&mut id, &mut translation, &mut contextual);
    messages
}

/// Decode one quoted `.po` string.
fn unquote(quoted: &str) -> String {
    let inner = quoted
        .trim()
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .unwrap_or_default();
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

/// The cultures the catalogues cover; kept in step with the selector.
#[cfg(test)]
fn catalogue_cultures() -> Vec<&'static str> {
    SOURCES.iter().map(|(culture, _)| *culture).collect()
}

/// Run `body` with `culture` as this thread's language, without a window.
/// For tests of strings Rust builds; the Slint bundle is not switched.
#[cfg(test)]
pub(crate) fn with_language<T>(culture: &'static str, body: impl FnOnce() -> T) -> T {
    let previous = CURRENT.with(|current| current.replace(culture));
    let result = body();
    CURRENT.with(|current| current.set(previous));
    result
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use d4lc_gui::settings::APP_LANGUAGES;

    #[test]
    fn catalogues_cover_exactly_the_selector_languages() {
        let offered: Vec<&str> = APP_LANGUAGES.iter().map(|language| language.id).collect();
        assert_eq!(catalogue_cultures(), offered);
    }

    #[test]
    fn parses_escapes_continuations_and_skips_untranslated_entries() {
        let messages = parse_po(
            "# comment\nmsgid \"\"\nmsgstr \"\"\n\"Language: de-DE\\n\"\n\n\
             #. rsCapA\nmsgid \"A \\\"q\\\"\"\nmsgstr \"B\\n\"\n\"C\"\n\n\
             msgid \"untranslated\"\nmsgstr \"\"\n\n\
             msgctxt \"d4lc\"\nmsgid \"A \\\"q\\\"\"\nmsgstr \"ctx\"\n",
        );
        // The contextual entry never answers a plain lookup.
        assert_eq!(messages.len(), 1);
        assert_eq!(messages["A \"q\""], "B\nC");
    }

    #[test]
    fn translates_upstream_strings_and_falls_back_to_english() {
        assert_eq!(current(), "en-US");
        assert_eq!(tr("Settings"), "Settings");
        with_language("de-DE", || {
            assert_eq!(tr("Settings"), "Einstellungen");
            // Upstream has no German value for this key: English stays.
            assert_eq!(tr("Top Most"), "Top Most");
            // Not an upstream string at all: passed through untouched.
            assert_eq!(tr("Not an upstream string"), "Not an upstream string");
        });
        with_language("fr-FR", || assert_eq!(tr("Settings"), "Paramètres"));
        with_language("zh-CN", || assert_eq!(tr("Settings"), "设置"));
        with_language("es-ES", || assert_eq!(tr("Top Most"), "Ventana Principal"));
        // zh-CN is translated in full; the port wrote the rows upstream lacks.
        with_language("zh-CN", || {
            assert_eq!(tr("Item Splitter Locations"), "物品分隔线位置");
            assert_eq!(tr("Cancel"), "取消");
        });
    }

    #[test]
    fn locales_map_to_the_resource_files_dotnet_would_find() {
        for (locale, culture) in [
            ("de_AT.UTF-8", Some("de-DE")),
            ("de-CH", Some("de-DE")),
            ("fr_CA", Some("fr-FR")),
            ("es_ES", Some("es-ES")),
            ("zh-CN", Some("zh-CN")),
            ("en_GB@euro", Some("en-US")),
            // Region-specific satellites do not serve other regions.
            ("es_MX", None),
            ("zh_TW", None),
            ("zh", None),
            ("C", None),
            ("", None),
        ] {
            assert_eq!(culture_for_locale(locale), culture, "{locale:?}");
        }
    }

    #[test]
    fn an_unknown_stored_culture_follows_the_system_locale_then_english() {
        assert_eq!(resolve_culture("fr-FR", Some("de_DE")), ("fr-FR", false));
        assert_eq!(
            resolve_culture("es-MX", Some("de_AT.UTF-8")),
            ("de-DE", true)
        );
        assert_eq!(resolve_culture("klingon", Some("ja_JP")), ("en-US", true));
        assert_eq!(resolve_culture("klingon", None), ("en-US", true));
    }

    #[test]
    fn the_selection_is_per_thread() {
        with_language("de-DE", || {
            let other = std::thread::spawn(|| (current(), tr("Settings")))
                .join()
                .unwrap_or_default();
            assert_eq!(other, ("en-US", "Settings".to_string()));
            assert_eq!(tr("Settings"), "Einstellungen");
        });
    }

    #[test]
    fn selecting_an_unknown_culture_is_refused_and_changes_nothing() {
        let error = select("es-MX").expect_err("es-MX is not offered");
        assert!(matches!(error, SelectError::UnknownCulture(_)), "{error}");
        assert_eq!(current(), "en-US");
    }

    /// The msgids of the `i18n::tr(...)` calls in a source file: a literal, or
    /// the value of a `const NAME: &str = "...";` of the same file.
    fn lookups_in(source: &str, path: &std::path::Path) -> Vec<String> {
        const CALL: &str = "i18n::tr(";
        let quoted = |text: &str| text[..text.find('"').unwrap_or(text.len())].to_string();
        source
            .match_indices(CALL)
            .map(|(start, _)| {
                let rest = &source[start + CALL.len()..];
                if let Some(literal) = rest.strip_prefix('"') {
                    return quoted(literal);
                }
                let name = &rest[..rest.find(')').unwrap_or(rest.len())];
                let declaration = format!("const {name}: &str = \"");
                let at = source.find(&declaration).unwrap_or_else(|| {
                    panic!("{path:?}: no literal or const for i18n::tr({name})")
                });
                quoted(&source[at + declaration.len()..])
            })
            .collect()
    }

    /// Every string Rust looks up must be an upstream string the catalogue
    /// carries: a typo would silently fall back to English forever.
    #[test]
    fn every_rust_side_lookup_is_an_upstream_string() {
        let english = &CATALOGUES["en-US"];
        let mut checked = 0;
        let mut pending = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
                let path = entry.unwrap_or_else(|e| panic!("{dir:?}: {e}")).path();
                if path.is_dir() {
                    pending.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && !path.ends_with("i18n.rs")
                {
                    let source =
                        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path:?}: {e}"));
                    for msgid in lookups_in(&source, &path) {
                        assert!(
                            english.contains_key(msgid.as_str()),
                            "{path:?}: i18n lookup of {msgid:?}, which is not an upstream string in the catalogue \
                             (add its key to translations/upstream-keys.txt)"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(
            checked > 0,
            "no Rust-side lookups found; the scan is broken"
        );
    }
}
