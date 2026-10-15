//! Binding model and parser: no X11 dependency, fully unit-tested.

/// Stable set of X11 modifier bits.
///
/// Mirrors a subset of `xproto::ModMask` but is pure Rust and
/// `Copy + Eq + Hash` so it can be used in tests without pulling
/// in the `x11rb` crate. The wire conversion is done in
/// `to_x11_modmask` at the call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct ModifierSet(u8);

impl ModifierSet {
    pub const SHIFT: Self = Self(1 << 0);
    pub const CTRL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2); // X11 Mod1
    pub const SUPER: Self = Self(1 << 3); // X11 Mod4
    pub const NUMLOCK: Self = Self(1 << 4);

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[cfg(test)]
    pub const fn bits(self) -> u8 {
        self.0
    }

    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }
}

impl std::ops::BitOr for ModifierSet {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for ModifierSet {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Normalized key token.
///
/// Each variant maps to a single X11 keysym. The mapping lives
/// in `named_key_to_keysym` and is exercised by the parser
/// tests; the keysym values are the public X11 constants
/// (XK_F5 = 0xffc2, XK_Tab = 0xff09, etc.).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NamedKey {
    // Function keys
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    // Letters (case-insensitive in the binding string;
    // canonical form is uppercase so Shift is recorded separately).
    Letter(char),
    // Digits
    Digit(u8),
    // Whitespace / control
    Tab,
    Enter,
    Space,
    Escape,
    Backspace,
    // Navigation block
    Insert,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
    // Arrow keys
    Up,
    Down,
    Left,
    Right,
    // Punctuation
    Minus,
    Equal,
    Comma,
    Period,
    Slash,
    Backslash,
    Semicolon,
    Apostrophe,
    BracketLeft,
    BracketRight,
    Grave,
}

/// Normalized binding parsed from a user-facing string.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ParsedBinding {
    pub modifiers: ModifierSet,
    pub key: NamedKey,
}

impl ParsedBinding {
    pub fn new(modifiers: ModifierSet, key: NamedKey) -> Self {
        Self { modifiers, key }
    }
}

/// Failure mode of the pure parser. Distinct from X11 errors
/// (those live in `HotkeyRegistrationStatus::Unavailable` and
/// `HotkeyRegistrationStatus::Conflict`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingParseError {
    /// Empty or whitespace-only input.
    Empty,
    /// No key token was supplied (e.g. `"Ctrl+"` or `"Ctrl+Shift"`).
    MissingKey,
    /// A token could not be recognized as a modifier or a key.
    /// The string payload is preserved verbatim for diagnostics.
    UnknownToken(String),
    /// The key token is recognized as a category but a specific
    /// instance is out of range (e.g. `"F13"`).
    UnsupportedKey(String),
    /// A letter token had more than one character (e.g. `"abc"`).
    InvalidLetter(String),
    /// A digit token had more than one character (e.g. `"42"`).
    InvalidDigit(String),
}

impl std::fmt::Display for BindingParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("binding string is empty"),
            Self::MissingKey => f.write_str("binding is missing a key token (e.g. \"Ctrl+F5\")"),
            Self::UnknownToken(t) => write!(f, "unknown binding token: {t:?}"),
            Self::UnsupportedKey(t) => write!(f, "unsupported key token: {t:?}"),
            Self::InvalidLetter(t) => write!(f, "invalid letter token: {t:?}"),
            Self::InvalidDigit(t) => write!(f, "invalid digit token: {t:?}"),
        }
    }
}

impl std::error::Error for BindingParseError {}

/// Parse a binding string into a normalized `ParsedBinding`.
///
/// Accepted forms (case-insensitive, `+` is the only separator):
///
/// - `"Ctrl+F5"`, `"ctrl+f5"`, `"CTRL + F5"` (whitespace around `+` ignored)
/// - `"Ctrl+Shift+F10"`, `"Alt+Shift+Tab"`
/// - `"F12"` (no modifier)
/// - `"A"`, `"a"` (letter, with Shift derived from case so the
///   canonical form is always upper-case key + explicit Shift modifier)
/// - `"0".."9"`
/// - `"Tab"`, `"Enter"`, `"Space"`, `"Escape"`, `"Backspace"`,
///   `"Insert"`, `"Delete"`, `"Home"`, `"End"`, `"PageUp"`,
///   `"PageDown"`, `"Up"`, `"Down"`, `"Left"`, `"Right"`,
///   punctuation tokens (`"-"`, `"="`, `","`, `"."`, `"/"`,
///   `"\\"`, `";"`, `"'"`, `"["`, `"]"`, `` "`" ``)
///
/// Recognized modifier tokens: `Ctrl`, `Control`, `Shift`, `Alt`,
/// `Meta`, `Super`, `Win`, `NumLock`. The mapping is one-way
/// (no aliasing of `Alt` ↔ `Mod1` etc. — the parser picks the
/// `Alt = Mod1` convention, the only one used by the upstream
/// `SettingsD4` defaults).
pub fn parse_binding(input: &str) -> Result<ParsedBinding, BindingParseError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(BindingParseError::Empty);
    }

    let mut modifiers = ModifierSet::default();
    let mut key_token: Option<&str> = None;

    for raw in trimmed.split('+') {
        let token = raw.trim();
        if token.is_empty() {
            return Err(BindingParseError::MissingKey);
        }
        match classify_token(token) {
            TokenClass::Modifier(m) => modifiers.insert(m),
            TokenClass::Key => {
                if key_token.is_some() {
                    // Multiple key tokens: e.g. "Ctrl+F5+F6" — reject.
                    return Err(BindingParseError::UnknownToken(token.to_string()));
                }
                key_token = Some(token);
            }
        }
    }

    let key_token = key_token.ok_or(BindingParseError::MissingKey)?;
    let key = parse_key_token(key_token)?;
    Ok(ParsedBinding::new(modifiers, key))
}

enum TokenClass {
    Modifier(ModifierSet),
    Key,
}

fn classify_token(token: &str) -> TokenClass {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => TokenClass::Modifier(ModifierSet::CTRL),
        "shift" => TokenClass::Modifier(ModifierSet::SHIFT),
        "alt" | "meta" => TokenClass::Modifier(ModifierSet::ALT),
        "super" | "win" => TokenClass::Modifier(ModifierSet::SUPER),
        "numlock" => TokenClass::Modifier(ModifierSet::NUMLOCK),
        _ => TokenClass::Key,
    }
}

fn parse_key_token(token: &str) -> Result<NamedKey, BindingParseError> {
    // 1. Function keys: F1..=F12. The "F" prefix check has to
    //    run before the named-keys match below, otherwise
    //    legitimate names like "F5" (uppercase F) could collide
    //    with the classifier heuristics. Function keys are
    //    detected in a case-insensitive way so "f5" / "F5" both
    //    work.
    //
    //    If the rest after the F prefix does not parse as a
    //    `u8` (e.g. `"Foo"` → rest `"oo"`), we deliberately do
    //    NOT return `UnsupportedKey` here — the token is not
    //    a function key at all, just a string that happens to
    //    start with the letter F. Fall through to the
    //    named-key / classifier path so the user gets the more
    //    accurate `UnknownToken` error variant.
    if let Some(rest) = token.strip_prefix('F').or_else(|| token.strip_prefix('f')) {
        if let Ok(n) = rest.parse::<u8>() {
            return match n {
                1 => Ok(NamedKey::F1),
                2 => Ok(NamedKey::F2),
                3 => Ok(NamedKey::F3),
                4 => Ok(NamedKey::F4),
                5 => Ok(NamedKey::F5),
                6 => Ok(NamedKey::F6),
                7 => Ok(NamedKey::F7),
                8 => Ok(NamedKey::F8),
                9 => Ok(NamedKey::F9),
                10 => Ok(NamedKey::F10),
                11 => Ok(NamedKey::F11),
                12 => Ok(NamedKey::F12),
                _ => Err(BindingParseError::UnsupportedKey(token.to_string())),
            };
        }
    }

    // 2. Single character: letter (A..Z, case-insensitive) or
    //    digit (0..9). Canonicalize the letter to upper-case so
    //    the binding string's case does not leak into the
    //    keysym lookup.
    if token.len() == 1 {
        // `len() == 1` guarantees a single UTF-8 byte; convert it
        // via the stable `u8 as char` cast to avoid panicking on
        // an empty iterator.
        let ch = token.as_bytes()[0] as char;
        if ch.is_ascii_alphabetic() {
            return Ok(NamedKey::Letter(ch.to_ascii_uppercase()));
        }
        if ch.is_ascii_digit() {
            return Ok(NamedKey::Digit((ch as u8) - b'0'));
        }
    }

    // 3. Named keys (case-insensitive). This MUST come before
    //    the multi-letter / multi-digit classifier below,
    //    otherwise legitimate names like "Tab" / "PageUp" /
    //    "Backspace" would be rejected as `InvalidLetter`
    //    before the named-key lookup ever runs. The lookup is
    //    exhaustive — a token that does not match a known name
    //    falls through to the classifier for an error.
    let lower = token.to_ascii_lowercase();
    if let Some(key) = lookup_named_key(&lower) {
        return Ok(key);
    }

    // 4. Classifier for the error variant. Distinguishes a
    //    "looks like a multi-letter typo" (e.g. `"abc"`) from
    //    a "looks like a name that isn't recognised"
    //    (e.g. `"Foo"`) and from a "looks like a multi-digit
    //    typo" (e.g. `"42"`). The split uses the first
    //    character's case because that matches the upstream
    //    convention: an initial uppercase looks like an
    //    attempted symbolic name (function key, named key,
    //    etc.), while an initial lowercase looks like an
    //    attempted single letter.
    let all_alpha = token.chars().all(|c| c.is_ascii_alphabetic());
    let all_digit = token.chars().all(|c| c.is_ascii_digit());
    let starts_uppercase = token
        .chars()
        .next()
        .map(|c| c.is_ascii_uppercase())
        .unwrap_or(false);

    if all_alpha && token.len() > 1 && !starts_uppercase {
        return Err(BindingParseError::InvalidLetter(token.to_string()));
    }
    if all_digit && token.len() > 1 {
        return Err(BindingParseError::InvalidDigit(token.to_string()));
    }
    Err(BindingParseError::UnknownToken(token.to_string()))
}

/// Look up a `NamedKey` from its canonical lowercase name.
/// Returns `None` for any token that is not a registered named
/// key — the caller (the parser) then decides which error
/// variant to surface.
fn lookup_named_key(lower: &str) -> Option<NamedKey> {
    Some(match lower {
        "tab" => NamedKey::Tab,
        "enter" | "return" => NamedKey::Enter,
        "space" => NamedKey::Space,
        "escape" | "esc" => NamedKey::Escape,
        "backspace" | "bs" => NamedKey::Backspace,
        "insert" | "ins" => NamedKey::Insert,
        "delete" | "del" => NamedKey::Delete,
        "home" => NamedKey::Home,
        "end" => NamedKey::End,
        "pageup" | "pgup" => NamedKey::PageUp,
        "pagedown" | "pgdn" => NamedKey::PageDown,
        "up" => NamedKey::Up,
        "down" => NamedKey::Down,
        "left" => NamedKey::Left,
        "right" => NamedKey::Right,
        "minus" | "-" => NamedKey::Minus,
        "equal" | "equals" | "=" => NamedKey::Equal,
        "comma" | "," => NamedKey::Comma,
        "period" | "." => NamedKey::Period,
        "slash" | "/" => NamedKey::Slash,
        "backslash" | "\\" => NamedKey::Backslash,
        "semicolon" | ";" => NamedKey::Semicolon,
        "apostrophe" | "quote" | "'" => NamedKey::Apostrophe,
        "bracketleft" | "[" => NamedKey::BracketLeft,
        "bracketright" | "]" => NamedKey::BracketRight,
        "grave" | "backtick" | "`" => NamedKey::Grave,
        _ => return None,
    })
}

/// Map a `NamedKey` to its X11 keysym value.
///
/// The constants are the public X11 keysyms (see
/// `<X11/keysym.h>` in the Xorg distribution). Kept inline to
/// avoid enabling the `x11rb-protocol` keysym feature.
pub fn named_key_to_keysym(key: NamedKey) -> u32 {
    match key {
        // Function keys (XK_F1 .. XK_F12 = 0xffbe .. 0xffc9)
        NamedKey::F1 => 0xffbe,
        NamedKey::F2 => 0xffbf,
        NamedKey::F3 => 0xffc0,
        NamedKey::F4 => 0xffc1,
        NamedKey::F5 => 0xffc2,
        NamedKey::F6 => 0xffc3,
        NamedKey::F7 => 0xffc4,
        NamedKey::F8 => 0xffc5,
        NamedKey::F9 => 0xffc6,
        NamedKey::F10 => 0xffc7,
        NamedKey::F11 => 0xffc8,
        NamedKey::F12 => 0xffc9,
        // Letters: XK_a..XK_z = 0x61..0x7a. Stored uppercase,
        // but the keysym is the lowercase form (X11 convention:
        // Shift modifier drives case in the event).
        NamedKey::Letter(c) => c.to_ascii_lowercase() as u32,
        // Digits: XK_0..XK_9 = 0x30..0x39
        NamedKey::Digit(d) => 0x30 + d as u32,
        NamedKey::Tab => 0xff09,
        NamedKey::Enter => 0xff0d,
        NamedKey::Space => 0x0020,
        NamedKey::Escape => 0xff1b,
        NamedKey::Backspace => 0xff08,
        NamedKey::Insert => 0xff63,
        NamedKey::Delete => 0xffff,
        NamedKey::Home => 0xff50,
        NamedKey::End => 0xff57,
        NamedKey::PageUp => 0xff55,
        NamedKey::PageDown => 0xff56,
        NamedKey::Up => 0xff52,
        NamedKey::Down => 0xff54,
        NamedKey::Left => 0xff51,
        NamedKey::Right => 0xff53,
        NamedKey::Minus => 0x002d,
        NamedKey::Equal => 0x003d,
        NamedKey::Comma => 0x002c,
        NamedKey::Period => 0x002e,
        NamedKey::Slash => 0x002f,
        NamedKey::Backslash => 0x005c,
        NamedKey::Semicolon => 0x003b,
        NamedKey::Apostrophe => 0x0027,
        NamedKey::BracketLeft => 0x005b,
        NamedKey::BracketRight => 0x005d,
        NamedKey::Grave => 0x0060,
    }
}

/// Convert a `ModifierSet` to the `u16` bitmask expected by
/// `x11rb`'s `ModMask::from_bits` (see `xproto::ModMask`).
pub fn to_x11_modmask_bits(modifiers: ModifierSet) -> u16 {
    let mut out: u16 = 0;
    if modifiers.contains(ModifierSet::SHIFT) {
        out |= 1 << 0; // ModMask::SHIFT
    }
    if modifiers.contains(ModifierSet::CTRL) {
        out |= 1 << 2; // ModMask::CONTROL
    }
    if modifiers.contains(ModifierSet::ALT) {
        out |= 1 << 3; // ModMask::M1
    }
    if modifiers.contains(ModifierSet::SUPER) {
        out |= 1 << 6; // ModMask::M4
    }
    if modifiers.contains(ModifierSet::NUMLOCK) {
        out |= 1 << 1; // ModMask::LOCK (NumLock is bit 1 on X11)
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;

    // -- Parser -------------------------------------------------------------

    #[test]
    fn parse_default_seven_bindings() {
        for (raw, expected_modifiers, expected_key) in [
            ("Ctrl+F5", ModifierSet::CTRL, NamedKey::F5),
            ("Ctrl+F6", ModifierSet::CTRL, NamedKey::F6),
            ("Ctrl+F10", ModifierSet::CTRL, NamedKey::F10),
            ("Ctrl+F9", ModifierSet::CTRL, NamedKey::F9),
            ("Ctrl+F12", ModifierSet::CTRL, NamedKey::F12),
            ("Ctrl+F11", ModifierSet::CTRL, NamedKey::F11),
            // Seventh upstream default binding.
            ("Ctrl+F7", ModifierSet::CTRL, NamedKey::F7),
        ] {
            let parsed = parse_binding(raw).expect(raw);
            assert_eq!(parsed.modifiers, expected_modifiers, "modifiers for {raw}");
            assert_eq!(parsed.key, expected_key, "key for {raw}");
        }
    }

    #[test]
    fn parse_binding_is_case_insensitive_for_modifiers() {
        let a = parse_binding("ctrl+f5").unwrap();
        let b = parse_binding("CTRL+F5").unwrap();
        let c = parse_binding("Ctrl + F5").unwrap();
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[test]
    fn parse_binding_with_shift_modifier() {
        let p = parse_binding("Ctrl+Shift+Tab").unwrap();
        assert_eq!(p.modifiers, ModifierSet::CTRL | ModifierSet::SHIFT);
        assert_eq!(p.key, NamedKey::Tab);
    }

    #[test]
    fn parse_binding_with_alt_is_mod1() {
        let p = parse_binding("Alt+F4").unwrap();
        assert_eq!(p.modifiers, ModifierSet::ALT);
        assert_eq!(p.key, NamedKey::F4);
    }

    #[test]
    fn parse_binding_with_super_is_mod4() {
        let p = parse_binding("Super+L").unwrap();
        assert_eq!(p.modifiers, ModifierSet::SUPER);
        assert_eq!(p.key, NamedKey::Letter('L'));
    }

    #[test]
    fn parse_binding_letter_is_uppercased() {
        let p = parse_binding("a").unwrap();
        assert_eq!(p.key, NamedKey::Letter('A'));
    }

    #[test]
    fn parse_binding_digit() {
        let p = parse_binding("Ctrl+5").unwrap();
        assert_eq!(p.key, NamedKey::Digit(5));
    }

    #[test]
    fn parse_binding_punctuation() {
        assert_eq!(parse_binding("-").unwrap().key, NamedKey::Minus);
        assert_eq!(parse_binding("=").unwrap().key, NamedKey::Equal);
        assert_eq!(parse_binding(",").unwrap().key, NamedKey::Comma);
        assert_eq!(parse_binding(".").unwrap().key, NamedKey::Period);
        assert_eq!(parse_binding("/").unwrap().key, NamedKey::Slash);
        assert_eq!(parse_binding("\\").unwrap().key, NamedKey::Backslash);
        assert_eq!(parse_binding(";").unwrap().key, NamedKey::Semicolon);
        assert_eq!(parse_binding("'").unwrap().key, NamedKey::Apostrophe);
        assert_eq!(parse_binding("[").unwrap().key, NamedKey::BracketLeft);
        assert_eq!(parse_binding("]").unwrap().key, NamedKey::BracketRight);
        assert_eq!(parse_binding("`").unwrap().key, NamedKey::Grave);
    }

    #[test]
    fn parse_binding_named_keys() {
        for (raw, key) in [
            ("Tab", NamedKey::Tab),
            ("Enter", NamedKey::Enter),
            ("Return", NamedKey::Enter),
            ("Space", NamedKey::Space),
            ("Escape", NamedKey::Escape),
            ("Esc", NamedKey::Escape),
            ("Backspace", NamedKey::Backspace),
            ("Insert", NamedKey::Insert),
            ("Delete", NamedKey::Delete),
            ("Home", NamedKey::Home),
            ("End", NamedKey::End),
            ("PageUp", NamedKey::PageUp),
            ("PgUp", NamedKey::PageUp),
            ("PageDown", NamedKey::PageDown),
            ("PgDn", NamedKey::PageDown),
            ("Up", NamedKey::Up),
            ("Down", NamedKey::Down),
            ("Left", NamedKey::Left),
            ("Right", NamedKey::Right),
        ] {
            assert_eq!(parse_binding(raw).unwrap().key, key, "{raw}");
        }
    }

    #[test]
    fn parse_binding_no_modifier() {
        let p = parse_binding("F12").unwrap();
        assert_eq!(p.modifiers, ModifierSet::default());
        assert_eq!(p.key, NamedKey::F12);
    }

    #[test]
    fn parse_binding_rejects_empty() {
        assert_eq!(parse_binding(""), Err(BindingParseError::Empty));
        assert_eq!(parse_binding("   "), Err(BindingParseError::Empty));
    }

    #[test]
    fn parse_binding_rejects_missing_key() {
        assert_eq!(parse_binding("Ctrl+"), Err(BindingParseError::MissingKey));
        assert_eq!(
            parse_binding("Ctrl+Shift"),
            Err(BindingParseError::MissingKey)
        );
    }

    #[test]
    fn parse_binding_rejects_unknown_token() {
        assert!(matches!(
            parse_binding("Ctrl+Foo"),
            Err(BindingParseError::UnknownToken(_))
        ));
    }

    #[test]
    fn parse_binding_rejects_f13() {
        assert_eq!(
            parse_binding("F13"),
            Err(BindingParseError::UnsupportedKey("F13".to_string()))
        );
    }

    #[test]
    fn parse_binding_rejects_multi_letter_word() {
        assert_eq!(
            parse_binding("Ctrl+abc"),
            Err(BindingParseError::InvalidLetter("abc".to_string()))
        );
    }

    #[test]
    fn parse_binding_rejects_multi_digit() {
        assert_eq!(
            parse_binding("Ctrl+42"),
            Err(BindingParseError::InvalidDigit("42".to_string()))
        );
    }

    #[test]
    fn parse_binding_rejects_two_key_tokens() {
        // "Ctrl+F5+F6" — only the first key token is kept, the
        // second is treated as an unknown modifier, so the
        // parser returns an error rather than silently
        // dropping a binding.
        assert!(matches!(
            parse_binding("Ctrl+F5+F6"),
            Err(BindingParseError::UnknownToken(_))
        ));
    }

    // -- Modifier ↔ x11 mask ------------------------------------------------

    #[test]
    fn modifier_set_contains_and_or() {
        let mut m = ModifierSet::default();
        m.insert(ModifierSet::CTRL);
        m.insert(ModifierSet::SHIFT);
        assert!(m.contains(ModifierSet::CTRL));
        assert!(m.contains(ModifierSet::SHIFT));
        assert!(!m.contains(ModifierSet::ALT));
        let union = ModifierSet::CTRL | ModifierSet::ALT;
        assert_eq!(
            union.bits(),
            ModifierSet::CTRL.bits() | ModifierSet::ALT.bits()
        );
    }

    #[test]
    fn to_x11_modmask_known_mappings() {
        // ShiftMask=1, LOCK=2, Control=4, M1=8, M4=64
        assert_eq!(to_x11_modmask_bits(ModifierSet::SHIFT), 1 << 0);
        assert_eq!(to_x11_modmask_bits(ModifierSet::NUMLOCK), 1 << 1);
        assert_eq!(to_x11_modmask_bits(ModifierSet::CTRL), 1 << 2);
        assert_eq!(to_x11_modmask_bits(ModifierSet::ALT), 1 << 3);
        assert_eq!(to_x11_modmask_bits(ModifierSet::SUPER), 1 << 6);
        let combined = ModifierSet::CTRL | ModifierSet::SHIFT | ModifierSet::ALT;
        assert_eq!(
            to_x11_modmask_bits(combined),
            (1 << 2) | (1 << 0) | (1 << 3)
        );
    }

    // -- Keysym values ------------------------------------------------------

    #[test]
    fn named_key_to_keysym_function_keys() {
        // F1 = 0xffbe, F12 = 0xffc9 (canonical X11 sequence).
        assert_eq!(named_key_to_keysym(NamedKey::F1), 0xffbe);
        assert_eq!(named_key_to_keysym(NamedKey::F5), 0xffc2);
        assert_eq!(named_key_to_keysym(NamedKey::F12), 0xffc9);
    }

    #[test]
    fn named_key_to_keysym_letters_use_lowercase() {
        // X11 keysyms for letters are lowercase; the Shift
        // modifier is recorded separately on the modifier
        // mask.
        assert_eq!(named_key_to_keysym(NamedKey::Letter('A')), 0x61);
        assert_eq!(named_key_to_keysym(NamedKey::Letter('z')), 0x7a);
    }

    #[test]
    fn named_key_to_keysym_digits() {
        assert_eq!(named_key_to_keysym(NamedKey::Digit(0)), 0x30);
        assert_eq!(named_key_to_keysym(NamedKey::Digit(9)), 0x39);
    }

    #[test]
    fn named_key_to_keysym_named_specials() {
        assert_eq!(named_key_to_keysym(NamedKey::Tab), 0xff09);
        assert_eq!(named_key_to_keysym(NamedKey::Enter), 0xff0d);
        assert_eq!(named_key_to_keysym(NamedKey::Space), 0x0020);
        assert_eq!(named_key_to_keysym(NamedKey::Escape), 0xff1b);
        assert_eq!(named_key_to_keysym(NamedKey::Backspace), 0xff08);
        assert_eq!(named_key_to_keysym(NamedKey::Up), 0xff52);
        assert_eq!(named_key_to_keysym(NamedKey::Down), 0xff54);
        assert_eq!(named_key_to_keysym(NamedKey::Left), 0xff51);
        assert_eq!(named_key_to_keysym(NamedKey::Right), 0xff53);
        assert_eq!(named_key_to_keysym(NamedKey::Delete), 0xffff);
    }
}
