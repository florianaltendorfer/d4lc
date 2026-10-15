//! `import-upstream-locales`: builds the app's gettext catalogues from the
//! upstream WPF resource files.
//!
//! Upstream keeps its UI strings in `D4Companion.Localization/Resources*.resx`
//! (an English base plus one satellite per culture) and binds them in XAML
//! through `{loc:LocExtension rsCap…}`. The port marks UI text with Slint's
//! `@tr`, whose msgid is the upstream English value. This module turns the
//! resx files into one `.po` per culture, restricted to the upstream keys the
//! port actually consumes (`translations/upstream-keys.txt`), so upstream stays
//! the only source of translated text.
//!
//! Strings the port renders that have no upstream key are collected into
//! `translations/port-only.txt` so the gap is explicit and reviewable. A
//! port-only string whose English happens to equal an upstream value must carry
//! the `d4lc` message context (`@tr("d4lc" => "Cancel")`), so it never resolves
//! through an upstream key it is not bound to.
//!
//! The one exception to "upstream is the only source of translated text" is
//! zh-CN, which the project owner translated in full: the strings upstream has
//! no zh-CN value for and the port-only strings come from
//! `translations/port/zh-CN.po`. Those rows are marked
//! `#. d4lc translation, not from upstream` in the generated catalogue.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Upstream resx directory, relative to the repository root.
const RESX_DIR: &str = "upstream/Diablo4Companion/D4Companion.Localization";
/// Catalogue root consumed by `slint_build::with_bundled_translations`.
const TRANSLATIONS_DIR: &str = "crates/d4lc-app/translations";
/// Hand-maintained list of the upstream keys the port consumes.
const KEY_MAP: &str = "crates/d4lc-app/translations/upstream-keys.txt";
/// Generated list of `@tr` strings that have no upstream key.
const PORT_ONLY: &str = "crates/d4lc-app/translations/port-only.txt";
/// Hand-written translations of the strings upstream has no value for.
const PORT_TRANSLATIONS_DIR: &str = "crates/d4lc-app/translations/port";
/// Message context of the port-only strings that collide with an upstream value.
const PORT_CONTEXT: &str = "d4lc";
/// Translator comment on every row the port, not upstream, translated.
const PORT_COMMENT: &str = "d4lc translation, not from upstream";
/// Slint sources scanned for `@tr` msgids.
const UI_DIR: &str = "crates/d4lc-app/ui";
/// The `.po` file stem; slint-build looks for `<crate name>.po`.
const DOMAIN: &str = "d4lc-app";

/// One UI language: the culture id upstream persists in `SelectedAppLanguage`
/// and the resx that carries its translations.
struct Culture {
    id: &'static str,
    resx: &'static str,
    /// The port's own translations in `translations/port`, for the culture
    /// the project translated beyond upstream. Such a culture is complete.
    own: Option<&'static str>,
}

/// The cultures the selector offers (`SettingsViewModel.cs:402-414`). en-US is
/// the neutral `Resources.resx`; the others resolve to their satellite the way
/// .NET resolves `new CultureInfo("de-DE")` to `Resources.de.resx`.
const CULTURES: [Culture; 5] = [
    Culture {
        id: "de-DE",
        resx: "Resources.de.resx",
        own: None,
    },
    Culture {
        id: "en-US",
        resx: "Resources.resx",
        own: None,
    },
    Culture {
        id: "es-ES",
        resx: "Resources.es-ES.resx",
        own: None,
    },
    Culture {
        id: "fr-FR",
        resx: "Resources.fr.resx",
        own: None,
    },
    Culture {
        id: "zh-CN",
        resx: "Resources.zh-CN.resx",
        own: Some("zh-CN.po"),
    },
];

type Error = Box<dyn std::error::Error>;

/// A file the importer owns, with the text it should contain.
pub struct Output {
    pub path: PathBuf,
    pub contents: String,
}

/// Translated values of one resx, in file order.
struct Resx {
    source: String,
    sha256: String,
    values: BTreeMap<String, String>,
    order: Vec<String>,
}

/// Parse a resx file's `<data name="…"><value>…</value></data>` rows.
///
/// Rows without a `<value>` (the Visual Studio template placeholders) and
/// rows carrying a `type`/`mimetype` (serialized objects) are not strings.
fn parse_resx(path: &Path, source: &str) -> Result<Resx, Error> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let text = String::from_utf8(bytes)?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let document = roxmltree::Document::parse(text)?;
    let mut values = BTreeMap::new();
    let mut order = Vec::new();
    for data in document
        .root_element()
        .children()
        .filter(|node| node.has_tag_name("data"))
    {
        if data.attribute("type").is_some() || data.attribute("mimetype").is_some() {
            continue;
        }
        let Some(name) = data.attribute("name") else {
            continue;
        };
        let Some(value) = data.children().find(|node| node.has_tag_name("value")) else {
            continue;
        };
        let value = value.text().unwrap_or_default().to_string();
        if values.insert(name.to_string(), value).is_some() {
            return Err(format!("{source}: duplicate key {name}").into());
        }
        order.push(name.to_string());
    }
    Ok(Resx {
        source: source.to_string(),
        sha256,
        values,
        order,
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// The consumed upstream keys, one per line; `#` starts a comment.
fn parse_key_map(text: &str) -> Result<Vec<String>, Error> {
    let mut keys = Vec::new();
    let mut seen = BTreeSet::new();
    for (index, line) in text.lines().enumerate() {
        let key = line.split('#').next().unwrap_or_default().trim();
        if key.is_empty() {
            continue;
        }
        if !seen.insert(key.to_string()) {
            return Err(format!("{KEY_MAP}:{}: duplicate key {key}", index + 1).into());
        }
        keys.push(key.to_string());
    }
    Ok(keys)
}

/// The upstream commit the resx files were read at.
fn upstream_revision(root: &Path) -> Result<String, Error> {
    let lock = fs::read_to_string(root.join("upstream.lock"))?;
    let field = |name: &str| {
        lock.lines().find_map(|line| {
            let (key, value) = line.split_once('=')?;
            (key.trim() == name).then(|| value.trim().trim_matches('"').to_string())
        })
    };
    match (
        field("last_reviewed_commit"),
        field("last_reviewed_release"),
    ) {
        (Some(commit), Some(release)) => Ok(format!("{commit} ({release})")),
        _ => Err("upstream.lock: missing last_reviewed_commit or last_reviewed_release".into()),
    }
}

/// Quote a string the way `.po` files do.
fn po_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// One catalogue row: the upstream keys sharing it (none for a port-only
/// string), its message context, the msgid (the neutral English value) and the
/// culture's translation.
struct Row<'a> {
    keys: &'a [&'a str],
    context: Option<&'a str>,
    msgid: &'a str,
    msgstr: Option<&'a str>,
    /// Translated by the port rather than upstream.
    port_authored: bool,
}

fn render_po(
    culture: &Culture,
    revision: &str,
    resx: &Resx,
    own: Option<&PortFile>,
    rows: &[Row<'_>],
) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Generated by `cargo xtask import-upstream-locales`; do not edit.\n\
         # Source: {RESX_DIR}/{}\n\
         # Source sha256: {}\n\
         # Upstream: {revision}",
        resx.source, resx.sha256
    );
    if let Some(own) = own {
        let _ = writeln!(
            out,
            "# Port translations: {PORT_TRANSLATIONS_DIR}/{}\n\
             # Port translations sha256: {}",
            own.name, own.sha256
        );
    }
    let _ = writeln!(
        out,
        "msgid \"\"\nmsgstr \"\"\n\
         \"Project-Id-Version: {DOMAIN}\\n\"\n\
         \"Language: {}\\n\"\n\
         \"MIME-Version: 1.0\\n\"\n\
         \"Content-Type: text/plain; charset=UTF-8\\n\"\n\
         \"Content-Transfer-Encoding: 8bit\\n\"",
        culture.id
    );
    for row in rows {
        out.push('\n');
        if !row.keys.is_empty() {
            let _ = writeln!(out, "#. {}", row.keys.join(", "));
        }
        if row.port_authored {
            let _ = writeln!(out, "#. {PORT_COMMENT}");
        }
        if let Some(context) = row.context {
            let _ = writeln!(out, "msgctxt {}", po_quote(context));
        }
        let _ = writeln!(
            out,
            "msgid {}\nmsgstr {}",
            po_quote(row.msgid),
            po_quote(row.msgstr.unwrap_or_default())
        );
    }
    out
}

/// One entry of a hand-written port translation file.
#[derive(Debug, PartialEq, Eq)]
struct PortEntry {
    context: Option<String>,
    msgid: String,
    msgstr: String,
}

/// The port's own translations of one culture.
struct PortFile {
    name: &'static str,
    sha256: String,
    entries: Vec<PortEntry>,
}

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

/// Parse a `.po` file into its entries (header excluded). Comments are free
/// text, so the translator can annotate a choice.
fn parse_port_entries(text: &str) -> Vec<PortEntry> {
    #[derive(Clone, Copy)]
    enum Field {
        Context,
        Id,
        Str,
    }
    let mut entries = Vec::new();
    let (mut context, mut id, mut string) = (None::<String>, String::new(), String::new());
    let mut field = None;
    let mut seen = false;
    let mut flush =
        |context: &mut Option<String>, id: &mut String, string: &mut String, seen: &mut bool| {
            if *seen && !id.is_empty() {
                entries.push(PortEntry {
                    context: context.take(),
                    msgid: std::mem::take(id),
                    msgstr: std::mem::take(string),
                });
            }
            *context = None;
            id.clear();
            string.clear();
            *seen = false;
        };
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("msgctxt ") {
            flush(&mut context, &mut id, &mut string, &mut seen);
            context = Some(unquote(rest));
            field = Some(Field::Context);
            seen = true;
        } else if let Some(rest) = line.strip_prefix("msgid ") {
            if !matches!(field, Some(Field::Context)) {
                flush(&mut context, &mut id, &mut string, &mut seen);
            }
            id = unquote(rest);
            field = Some(Field::Id);
            seen = true;
        } else if let Some(rest) = line.strip_prefix("msgstr ") {
            string = unquote(rest);
            field = Some(Field::Str);
        } else if line.starts_with('"') {
            match field {
                Some(Field::Context) => context
                    .get_or_insert_with(String::new)
                    .push_str(&unquote(line)),
                Some(Field::Id) => id.push_str(&unquote(line)),
                Some(Field::Str) => string.push_str(&unquote(line)),
                None => {}
            }
        } else if line.is_empty() {
            flush(&mut context, &mut id, &mut string, &mut seen);
            field = None;
        }
    }
    flush(&mut context, &mut id, &mut string, &mut seen);
    entries
}

fn read_port_file(root: &Path, name: &'static str) -> Result<PortFile, Error> {
    let path = root.join(PORT_TRANSLATIONS_DIR).join(name);
    let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let sha256 = hex(&Sha256::digest(&bytes));
    let entries = parse_port_entries(&String::from_utf8(bytes)?);
    Ok(PortFile {
        name,
        sha256,
        entries,
    })
}

/// One `@tr(...)` call: its optional message context and its msgid.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct TrCall {
    pub context: Option<String>,
    pub msgid: String,
}

/// Decode the `@tr(...)` calls in a Slint source.
///
/// The scan honours comments and string literals, including `\{expr}`
/// interpolations (which hold code, and so may hold `@tr`), so an `@tr(` inside
/// a comment or plain string is not mistaken for a call. The plain form
/// (`@tr("text", args…)`) and the context form (`@tr("ctx" => "text", …)` are
/// supported; plurals are not, the catalogues carry none.
pub fn extract_tr_calls(source: &str, file: &str) -> Result<Vec<TrCall>, Error> {
    let chars: Vec<char> = source.chars().collect();
    let mut found = Vec::new();
    let mut scanner = Scanner {
        chars: &chars,
        at: 0,
        file,
    };
    scanner.code(false, &mut found)?;
    Ok(found)
}

struct Scanner<'a> {
    chars: &'a [char],
    at: usize,
    file: &'a str,
}

impl Scanner<'_> {
    fn peek(&self, offset: usize) -> Option<char> {
        self.chars.get(self.at + offset).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.peek(0).is_some_and(char::is_whitespace) {
            self.at += 1;
        }
    }

    /// Scan code; with `in_interpolation`, stop after the closing `}` of a
    /// string interpolation.
    fn code(&mut self, in_interpolation: bool, found: &mut Vec<TrCall>) -> Result<(), Error> {
        let mut depth = 0usize;
        while let Some(ch) = self.peek(0) {
            match ch {
                '/' if self.peek(1) == Some('/') => {
                    while self.peek(0).is_some_and(|ch| ch != '\n') {
                        self.at += 1;
                    }
                }
                '/' if self.peek(1) == Some('*') => {
                    self.at += 2;
                    while self.peek(0).is_some()
                        && !(self.peek(0) == Some('*') && self.peek(1) == Some('/'))
                    {
                        self.at += 1;
                    }
                    self.at += 2;
                }
                '"' => {
                    self.at += 1;
                    self.string(found)?;
                }
                '{' => {
                    depth += 1;
                    self.at += 1;
                }
                '}' if in_interpolation && depth == 0 => {
                    self.at += 1;
                    return Ok(());
                }
                '}' => {
                    depth = depth.saturating_sub(1);
                    self.at += 1;
                }
                '@' if self.starts_with("@tr(") => {
                    self.at += 4;
                    self.skip_whitespace();
                    if self.peek(0) != Some('"') {
                        return Err(format!("{}: @tr without a string literal", self.file).into());
                    }
                    let first = self.msgid()?;
                    self.skip_whitespace();
                    if self.peek(0) == Some('=') && self.peek(1) == Some('>') {
                        self.at += 2;
                        self.skip_whitespace();
                        if self.peek(0) != Some('"') {
                            return Err(format!(
                                "{}: @tr context without a string literal",
                                self.file
                            )
                            .into());
                        }
                        let msgid = self.msgid()?;
                        found.push(TrCall {
                            context: Some(first),
                            msgid,
                        });
                    } else if self.peek(0) == Some('|') {
                        return Err(format!(
                            "{}: @tr(\"{first}\") uses a plural form; the catalogues have none",
                            self.file
                        )
                        .into());
                    } else {
                        found.push(TrCall {
                            context: None,
                            msgid: first,
                        });
                    }
                }
                _ => self.at += 1,
            }
        }
        if in_interpolation {
            return Err(format!("{}: unterminated string interpolation", self.file).into());
        }
        Ok(())
    }

    fn starts_with(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(i, ch)| self.peek(i) == Some(ch))
    }

    /// Skip a string literal whose opening quote is consumed, scanning its
    /// interpolations for `@tr` calls.
    fn string(&mut self, found: &mut Vec<TrCall>) -> Result<(), Error> {
        while let Some(ch) = self.peek(0) {
            match ch {
                '"' => {
                    self.at += 1;
                    return Ok(());
                }
                '\\' if self.peek(1) == Some('{') => {
                    self.at += 2;
                    self.code(true, found)?;
                }
                '\\' => self.at += 2,
                _ => self.at += 1,
            }
        }
        Err(format!("{}: unterminated string literal", self.file).into())
    }

    /// Decode the plain string literal at the cursor (a `@tr` msgid).
    fn msgid(&mut self) -> Result<String, Error> {
        let file = self.file;
        self.at += 1;
        let mut out = String::new();
        while let Some(ch) = self.peek(0) {
            self.at += 1;
            match ch {
                '"' => return Ok(out),
                '\\' => {
                    let escaped = self.peek(0);
                    self.at += 1;
                    match escaped {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('n') => out.push('\n'),
                        Some('r') => out.push('\r'),
                        Some('t') => out.push('\t'),
                        Some('u') => {
                            let mut digits = String::new();
                            while let Some(next) = self.peek(0) {
                                self.at += 1;
                                if next == '}' {
                                    break;
                                }
                                if next != '{' {
                                    digits.push(next);
                                }
                            }
                            let decoded = u32::from_str_radix(&digits, 16)
                                .ok()
                                .and_then(char::from_u32)
                                .ok_or_else(|| format!("{file}: bad \\u escape"))?;
                            out.push(decoded);
                        }
                        other => {
                            return Err(format!(
                                "{file}: unsupported escape in a @tr msgid: \\{other:?}"
                            )
                            .into())
                        }
                    }
                }
                ch => out.push(ch),
            }
        }
        Err(format!("{file}: unterminated @tr msgid").into())
    }
}

/// Every `.slint` file under `dir`, sorted.
fn slint_files(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == "slint") {
                files.push(path);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// All `@tr` calls of the shipped UI, de-duplicated and sorted.
pub fn ui_calls(root: &Path) -> Result<BTreeSet<TrCall>, Error> {
    let mut all = BTreeSet::new();
    for path in slint_files(&root.join(UI_DIR))? {
        let source = fs::read_to_string(&path)?;
        all.extend(extract_tr_calls(&source, &path.display().to_string())?);
    }
    Ok(all)
}

/// The generated files plus how much of the consumed vocabulary each culture
/// translates.
pub struct Generated {
    pub outputs: Vec<Output>,
    /// `(culture id, translated rows, total rows)`.
    pub coverage: Vec<(&'static str, usize, usize)>,
}

/// Remove and return the translation of `(context, msgid)` from `unused`.
fn take_entry<'a>(
    unused: &mut Vec<&'a PortEntry>,
    context: Option<&str>,
    msgid: &str,
) -> Option<&'a str> {
    let at = unused
        .iter()
        .position(|entry| entry.context.as_deref() == context && entry.msgid == msgid)?;
    let entry = unused.remove(at);
    Some(entry.msgstr.as_str()).filter(|text| !text.is_empty())
}

/// Build every file the importer owns.
pub fn generate(root: &Path) -> Result<Generated, Error> {
    let keys = parse_key_map(&fs::read_to_string(root.join(KEY_MAP))?)?;
    let revision = upstream_revision(root)?;
    let resx_dir = root.join(RESX_DIR);
    let neutral = parse_resx(&resx_dir.join("Resources.resx"), "Resources.resx")?;
    for key in &keys {
        match neutral.values.get(key) {
            None => return Err(format!("{KEY_MAP}: {key} is not an upstream resource key").into()),
            Some(value) if value.is_empty() => {
                return Err(format!("{KEY_MAP}: {key} has an empty English value").into())
            }
            Some(_) => {}
        }
    }
    // One msgid can be reached through several keys (`rsCapSeasonal` and
    // `rsTooltipSeasonal`). Such keys share one row and must agree on every
    // translation. The rows follow the upstream file order, not the map's
    // order, so a reshuffled map does not churn the generated files.
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for key in neutral.order.iter().filter(|key| keys.contains(key)) {
        let msgid = neutral.values[key].as_str();
        match groups.iter_mut().find(|(known, _)| *known == msgid) {
            Some((_, members)) => members.push(key),
            None => groups.push((msgid, vec![key])),
        }
    }
    let upstream_owned: BTreeSet<&str> = groups.iter().map(|(msgid, _)| *msgid).collect();
    let upstream_values: BTreeSet<&str> = neutral.values.values().map(String::as_str).collect();

    // The port-only strings: no upstream key binds them. A plain one must not
    // collide with any upstream English value, or the lookup would hand it that
    // value's translation.
    let mut port_only: Vec<TrCall> = Vec::new();
    for call in ui_calls(root)? {
        match &call.context {
            None if upstream_owned.contains(call.msgid.as_str()) => {}
            None if upstream_values.contains(call.msgid.as_str()) => {
                return Err(format!(
                    "@tr({:?}) is not bound to an upstream key but equals an upstream English \
                     value; mark it @tr({PORT_CONTEXT:?} => {:?}) or add the key to {KEY_MAP}",
                    call.msgid, call.msgid
                )
                .into());
            }
            Some(context) if context != PORT_CONTEXT => {
                return Err(format!(
                    "@tr({context:?} => {:?}): the only message context is {PORT_CONTEXT:?}",
                    call.msgid
                )
                .into());
            }
            _ => port_only.push(call),
        }
    }

    let mut outputs = Vec::new();
    let mut coverage = Vec::new();
    for culture in &CULTURES {
        let satellite = if culture.resx == "Resources.resx" {
            None
        } else {
            Some(parse_resx(&resx_dir.join(culture.resx), culture.resx)?)
        };
        let source = satellite.as_ref().unwrap_or(&neutral);
        let own = culture
            .own
            .map(|name| read_port_file(root, name))
            .transpose()?;
        let mut unused: Vec<&PortEntry> = own.iter().flat_map(|file| &file.entries).collect();

        let mut rows = Vec::new();
        for (msgid, members) in &groups {
            let mut msgstr: Option<&str> = None;
            for member in members {
                let candidate = match &satellite {
                    None => Some(*msgid),
                    Some(resx) => resx.values.get(*member).map(String::as_str),
                };
                match (msgstr, candidate) {
                    (Some(first), Some(other)) if first != other => {
                        return Err(format!(
                            "{}: {members:?} share the msgid {msgid:?} but translate it differently; \
                             consume only one of them",
                            culture.id
                        )
                        .into());
                    }
                    (None, other) => msgstr = other,
                    _ => {}
                }
            }
            let mut port_authored = false;
            if culture.own.is_some() {
                let own_text = take_entry(&mut unused, None, msgid);
                match (msgstr, own_text) {
                    (Some(_), Some(_)) => {
                        return Err(format!(
                            "{}: {msgid:?} already has an upstream translation; drop it from {PORT_TRANSLATIONS_DIR}",
                            culture.id
                        )
                        .into());
                    }
                    (None, Some(text)) => {
                        msgstr = Some(text);
                        port_authored = true;
                    }
                    (None, None) => {
                        return Err(format!(
                            "{}: no translation for {msgid:?} in {PORT_TRANSLATIONS_DIR}",
                            culture.id
                        )
                        .into());
                    }
                    (Some(_), None) => {}
                }
            }
            rows.push(Row {
                keys: members,
                context: None,
                msgid,
                msgstr,
                port_authored,
            });
        }
        if culture.own.is_some() {
            for call in &port_only {
                let Some(text) = take_entry(&mut unused, call.context.as_deref(), &call.msgid)
                else {
                    return Err(format!(
                        "{}: no translation for port-only {:?} in {PORT_TRANSLATIONS_DIR}",
                        culture.id, call.msgid
                    )
                    .into());
                };
                rows.push(Row {
                    keys: &[],
                    context: call.context.as_deref(),
                    msgid: &call.msgid,
                    msgstr: Some(text),
                    port_authored: true,
                });
            }
            if let Some(entry) = unused.first() {
                return Err(format!(
                    "{}: {PORT_TRANSLATIONS_DIR} has an entry the UI does not use: {:?}",
                    culture.id, entry.msgid
                )
                .into());
            }
        }
        let translated = rows.iter().filter(|row| row.msgstr.is_some()).count();
        coverage.push((culture.id, translated, rows.len()));
        outputs.push(Output {
            path: root
                .join(TRANSLATIONS_DIR)
                .join(culture.id)
                .join("LC_MESSAGES")
                .join(format!("{DOMAIN}.po")),
            contents: render_po(culture, &revision, source, own.as_ref(), &rows),
        });
    }

    let mut gap = String::from(
        "# Generated by `cargo xtask import-upstream-locales`; do not edit.\n\
         # @tr strings in crates/d4lc-app/ui with no upstream resource key. Upstream either\n\
         # hard-codes them or has no such control, so only zh-CN (translated by the port) has them.\n",
    );
    for call in &port_only {
        match &call.context {
            Some(context) => {
                let _ = writeln!(gap, "{} => {}", po_quote(context), po_quote(&call.msgid));
            }
            None => {
                let _ = writeln!(gap, "{}", po_quote(&call.msgid));
            }
        }
    }
    outputs.push(Output {
        path: root.join(PORT_ONLY),
        contents: gap,
    });
    Ok(Generated { outputs, coverage })
}

/// Write the generated files, or with `check` verify they are up to date.
pub fn run(root: &Path, check: bool) -> Result<(), Error> {
    let generated = generate(root)?;
    let mut stale = Vec::new();
    for output in &generated.outputs {
        if check {
            if fs::read_to_string(&output.path).ok().as_deref() != Some(output.contents.as_str()) {
                stale.push(output.path.display().to_string());
            }
        } else {
            if let Some(parent) = output.path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&output.path, &output.contents)?;
        }
    }
    if !stale.is_empty() {
        return Err(format!(
            "stale generated files (run `cargo xtask import-upstream-locales`): {}",
            stale.join(", ")
        )
        .into());
    }
    for (id, translated, total) in generated.coverage {
        println!("{id}: {translated}/{total} rows translated");
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn extracts_plain_tr_calls_only_outside_comments_and_strings() {
        let source = r#"
            // @tr("commented")
            /* @tr("block") */
            Text { text: @tr("Settings"); tooltip: "@tr(\"in a string\")"; }
            Text { text: @tr("Set color: {}", name); }
            Text { text: @tr("Quote \"x\"\n"); }
            Tooltip { text: @markdown("\{@tr("Nested")}"); }
        "#;
        assert_eq!(
            extract_tr_calls(source, "t.slint")
                .unwrap()
                .into_iter()
                .map(|call| call.msgid)
                .collect::<Vec<_>>(),
            ["Settings", "Set color: {}", "Quote \"x\"\n", "Nested"]
        );
    }

    #[test]
    fn reads_the_context_form_and_rejects_plurals() {
        let calls =
            extract_tr_calls(r#"@tr("d4lc" => "Cancel", x) @tr("Plain")"#, "t.slint").unwrap();
        assert_eq!(
            calls,
            [
                TrCall {
                    context: Some("d4lc".to_string()),
                    msgid: "Cancel".to_string()
                },
                TrCall {
                    context: None,
                    msgid: "Plain".to_string()
                },
            ]
        );
        assert!(extract_tr_calls(r#"@tr("one" | "many" % n)"#, "t.slint").is_err());
        assert!(extract_tr_calls("@tr(name)", "t.slint").is_err());
        assert!(extract_tr_calls(r#"@tr("ctx" => name)"#, "t.slint").is_err());
    }

    #[test]
    fn port_entries_keep_context_and_skip_the_header() {
        let entries = parse_port_entries(
            "# note\nmsgid \"\"\nmsgstr \"\"\n\"Language: zh-CN\\n\"\n\n\
             msgid \"A\"\nmsgstr \"甲\"\n\n\
             # free comment\nmsgctxt \"d4lc\"\nmsgid \"B \\\"q\\\"\"\nmsgstr \"乙\"\n\"丙\"\n",
        );
        assert_eq!(
            entries,
            [
                PortEntry {
                    context: None,
                    msgid: "A".to_string(),
                    msgstr: "甲".to_string()
                },
                PortEntry {
                    context: Some("d4lc".to_string()),
                    msgid: "B \"q\"".to_string(),
                    msgstr: "乙丙".to_string()
                },
            ]
        );
    }

    #[test]
    fn po_quote_round_trips_through_the_extractor() {
        let text = "a \"b\" \\ c\n\td";
        let quoted = po_quote(text);
        let decoded = extract_tr_calls(&format!("@tr({quoted})"), "t.slint").unwrap();
        assert_eq!(decoded[0].msgid, text);
    }

    #[test]
    fn key_map_ignores_comments_and_rejects_duplicates() {
        let keys = parse_key_map("# header\nrsCapA  # note\n\nrsCapB\n").unwrap();
        assert_eq!(keys, ["rsCapA", "rsCapB"]);
        assert!(parse_key_map("rsCapA\nrsCapA\n").is_err());
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod repo_tests;
