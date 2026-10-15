//! The `KeyBindingConfig` JSON shape upstream persists for a hotkey, and the
//! conversion to and from the binding string the rest of d4lc uses.
//!
//! Upstream stores each hotkey as a `KeyBindingConfig` object
//! (`D4Companion.Entities/KeyBindingConfig.cs:8-14`) on `SettingsD4`
//! (`SettingsD4.cs:93-153`, one property per target) and writes it with
//! `System.Text.Json` defaults (`SettingsManager.cs:66`). That serializer
//! writes enums as their integer value, so `KeyGestureKey` is the integer of
//! WPF `System.Windows.Input.Key` and `KeyGestureModifier` the integer of
//! `System.Windows.Input.ModifierKeys`:
//!
//! ```json
//! "KeyBindingConfigSwitchPreset": {
//!   "IsEnabled": false,
//!   "Name": "Switch Preset",
//!   "KeyGestureKey": 94,
//!   "KeyGestureModifier": 2
//! }
//! ```
//!
//! `ModifierKeys` is a flags enum: `Alt = 1`, `Control = 2`, `Shift = 4`,
//! `Windows = 8` (dotnet/wpf `WindowsBase/System/Windows/Input/
//! ModifierKeys.cs`). `Key` has no explicit values: members take the next
//! integer in declaration order and an alias (`Enter = Return`) repeats the
//! value of its target (dotnet/wpf `WindowsBase/System/Windows/Input/Key.cs`).
//! [`WPF_KEYS`] lists every member in declaration order, derived from that
//! file, so `F5` is 94 and `Control` + `F5` is the upstream default of
//! "Switch Preset".

/// `(d4lc target name, upstream property, upstream `Name`)` for every
/// `KeyBindingConfig*` property of `SettingsD4.cs:99-153`.
pub(super) const UPSTREAM_HOTKEYS: &[(&str, &str, &str)] = &[
    (
        "SwitchPreset",
        "KeyBindingConfigSwitchPreset",
        "Switch Preset",
    ),
    (
        "SwitchOverlay",
        "KeyBindingConfigSwitchOverlay",
        "Switch Overlay",
    ),
    (
        "TakeScreenshot",
        "KeyBindingConfigTakeScreenshot",
        "Take Screenshot",
    ),
    (
        "ToggleController",
        "KeyBindingConfigToggleController",
        "Toggle Controller",
    ),
    (
        "ToggleTuningPrismHints",
        "KeyBindingConfigToggleTuningPrismHints",
        "Toggle Tuning Prism Hints",
    ),
    (
        "ToggleOverlay",
        "KeyBindingConfigToggleOverlay",
        "Toggle Overlay",
    ),
    (
        "ToggleDebugLockScreencapture",
        "KeyBindingConfigToggleDebugLockScreencapture",
        "Debug: Toggle Lock Screencapture",
    ),
];

const MODIFIER_ALT: i32 = 1;
const MODIFIER_CONTROL: i32 = 2;
const MODIFIER_SHIFT: i32 = 4;
const MODIFIER_WINDOWS: i32 = 8;

/// Every member of WPF `System.Windows.Input.Key` with its integer value, in
/// declaration order (aliases included).
const WPF_KEYS: &[(&str, i32)] = &[
    ("None", 0),
    ("Cancel", 1),
    ("Back", 2),
    ("Tab", 3),
    ("LineFeed", 4),
    ("Clear", 5),
    ("Return", 6),
    ("Enter", 6),
    ("Pause", 7),
    ("Capital", 8),
    ("CapsLock", 8),
    ("KanaMode", 9),
    ("HangulMode", 9),
    ("JunjaMode", 10),
    ("FinalMode", 11),
    ("HanjaMode", 12),
    ("KanjiMode", 12),
    ("Escape", 13),
    ("ImeConvert", 14),
    ("ImeNonConvert", 15),
    ("ImeAccept", 16),
    ("ImeModeChange", 17),
    ("Space", 18),
    ("Prior", 19),
    ("PageUp", 19),
    ("Next", 20),
    ("PageDown", 20),
    ("End", 21),
    ("Home", 22),
    ("Left", 23),
    ("Up", 24),
    ("Right", 25),
    ("Down", 26),
    ("Select", 27),
    ("Print", 28),
    ("Execute", 29),
    ("Snapshot", 30),
    ("PrintScreen", 30),
    ("Insert", 31),
    ("Delete", 32),
    ("Help", 33),
    ("D0", 34),
    ("D1", 35),
    ("D2", 36),
    ("D3", 37),
    ("D4", 38),
    ("D5", 39),
    ("D6", 40),
    ("D7", 41),
    ("D8", 42),
    ("D9", 43),
    ("A", 44),
    ("B", 45),
    ("C", 46),
    ("D", 47),
    ("E", 48),
    ("F", 49),
    ("G", 50),
    ("H", 51),
    ("I", 52),
    ("J", 53),
    ("K", 54),
    ("L", 55),
    ("M", 56),
    ("N", 57),
    ("O", 58),
    ("P", 59),
    ("Q", 60),
    ("R", 61),
    ("S", 62),
    ("T", 63),
    ("U", 64),
    ("V", 65),
    ("W", 66),
    ("X", 67),
    ("Y", 68),
    ("Z", 69),
    ("LWin", 70),
    ("RWin", 71),
    ("Apps", 72),
    ("Sleep", 73),
    ("NumPad0", 74),
    ("NumPad1", 75),
    ("NumPad2", 76),
    ("NumPad3", 77),
    ("NumPad4", 78),
    ("NumPad5", 79),
    ("NumPad6", 80),
    ("NumPad7", 81),
    ("NumPad8", 82),
    ("NumPad9", 83),
    ("Multiply", 84),
    ("Add", 85),
    ("Separator", 86),
    ("Subtract", 87),
    ("Decimal", 88),
    ("Divide", 89),
    ("F1", 90),
    ("F2", 91),
    ("F3", 92),
    ("F4", 93),
    ("F5", 94),
    ("F6", 95),
    ("F7", 96),
    ("F8", 97),
    ("F9", 98),
    ("F10", 99),
    ("F11", 100),
    ("F12", 101),
    ("F13", 102),
    ("F14", 103),
    ("F15", 104),
    ("F16", 105),
    ("F17", 106),
    ("F18", 107),
    ("F19", 108),
    ("F20", 109),
    ("F21", 110),
    ("F22", 111),
    ("F23", 112),
    ("F24", 113),
    ("NumLock", 114),
    ("Scroll", 115),
    ("LeftShift", 116),
    ("RightShift", 117),
    ("LeftCtrl", 118),
    ("RightCtrl", 119),
    ("LeftAlt", 120),
    ("RightAlt", 121),
    ("BrowserBack", 122),
    ("BrowserForward", 123),
    ("BrowserRefresh", 124),
    ("BrowserStop", 125),
    ("BrowserSearch", 126),
    ("BrowserFavorites", 127),
    ("BrowserHome", 128),
    ("VolumeMute", 129),
    ("VolumeDown", 130),
    ("VolumeUp", 131),
    ("MediaNextTrack", 132),
    ("MediaPreviousTrack", 133),
    ("MediaStop", 134),
    ("MediaPlayPause", 135),
    ("LaunchMail", 136),
    ("SelectMedia", 137),
    ("LaunchApplication1", 138),
    ("LaunchApplication2", 139),
    ("Oem1", 140),
    ("OemSemicolon", 140),
    ("OemPlus", 141),
    ("OemComma", 142),
    ("OemMinus", 143),
    ("OemPeriod", 144),
    ("Oem2", 145),
    ("OemQuestion", 145),
    ("Oem3", 146),
    ("OemTilde", 146),
    ("AbntC1", 147),
    ("AbntC2", 148),
    ("Oem4", 149),
    ("OemOpenBrackets", 149),
    ("Oem5", 150),
    ("OemPipe", 150),
    ("Oem6", 151),
    ("OemCloseBrackets", 151),
    ("Oem7", 152),
    ("OemQuotes", 152),
    ("Oem8", 153),
    ("Oem102", 154),
    ("OemBackslash", 154),
    ("ImeProcessed", 155),
    ("System", 156),
    ("OemAttn", 157),
    ("DbeAlphanumeric", 157),
    ("OemFinish", 158),
    ("DbeKatakana", 158),
    ("OemCopy", 159),
    ("DbeHiragana", 159),
    ("OemAuto", 160),
    ("DbeSbcsChar", 160),
    ("OemEnlw", 161),
    ("DbeDbcsChar", 161),
    ("OemBackTab", 162),
    ("DbeRoman", 162),
    ("Attn", 163),
    ("DbeNoRoman", 163),
    ("CrSel", 164),
    ("DbeEnterWordRegisterMode", 164),
    ("ExSel", 165),
    ("DbeEnterImeConfigureMode", 165),
    ("EraseEof", 166),
    ("DbeFlushString", 166),
    ("Play", 167),
    ("DbeCodeInput", 167),
    ("Zoom", 168),
    ("DbeNoCodeInput", 168),
    ("NoName", 169),
    ("DbeDetermineString", 169),
    ("Pa1", 170),
    ("DbeEnterDialogConversionMode", 170),
    ("OemClear", 171),
    ("DeadCharProcessed", 172),
];

/// The binding-string token d4lc uses for the keys it names itself, keyed by
/// WPF `Key` name. Every other key round-trips under its WPF name.
const BINDING_TOKENS: &[(&str, &str)] = &[
    ("Return", "Enter"),
    ("Back", "Backspace"),
    ("Prior", "PageUp"),
    ("Next", "PageDown"),
    ("OemMinus", "Minus"),
    ("OemPlus", "Equal"),
    ("OemComma", "Comma"),
    ("OemPeriod", "Period"),
    ("OemQuestion", "Slash"),
    ("OemPipe", "Backslash"),
    ("OemSemicolon", "Semicolon"),
    ("OemQuotes", "Apostrophe"),
    ("OemOpenBrackets", "BracketLeft"),
    ("OemCloseBrackets", "BracketRight"),
    ("OemTilde", "Grave"),
];

/// Binding-string spellings that name a WPF key differently from
/// [`BINDING_TOKENS`] (the aliases `parse_binding` accepts).
const BINDING_ALIASES: &[(&str, &str)] = &[
    ("esc", "Escape"),
    ("bs", "Back"),
    ("backspace", "Back"),
    ("ins", "Insert"),
    ("del", "Delete"),
    ("pgup", "PageUp"),
    ("pgdn", "PageDown"),
    ("equals", "OemPlus"),
    ("quote", "OemQuotes"),
    ("backtick", "OemTilde"),
    ("-", "OemMinus"),
    ("=", "OemPlus"),
    (",", "OemComma"),
    (".", "OemPeriod"),
    ("/", "OemQuestion"),
    ("\\", "OemPipe"),
    (";", "OemSemicolon"),
    ("'", "OemQuotes"),
    ("[", "OemOpenBrackets"),
    ("]", "OemCloseBrackets"),
    ("`", "OemTilde"),
];

fn key_value(wpf_name: &str) -> Option<i32> {
    WPF_KEYS
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wpf_name))
        .map(|(_, value)| *value)
}

/// The WPF `Key` integer for the key token of a binding string.
fn key_for_token(token: &str) -> Option<i32> {
    if let [only] = token.as_bytes() {
        if only.is_ascii_digit() {
            return key_value(&format!("D{}", *only as char));
        }
    }
    if let Some((wpf_name, _)) = BINDING_TOKENS
        .iter()
        .find(|(_, binding)| binding.eq_ignore_ascii_case(token))
    {
        return key_value(wpf_name);
    }
    if let Some((_, wpf_name)) = BINDING_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(token))
    {
        return key_value(wpf_name);
    }
    key_value(token)
}

/// The binding-string token for a WPF `Key` integer.
fn token_for_key(value: i32) -> Option<String> {
    let (name, _) = WPF_KEYS.iter().find(|(_, candidate)| *candidate == value)?;
    if let Some(digit) = name.strip_prefix('D').filter(|rest| rest.len() == 1) {
        if digit.as_bytes()[0].is_ascii_digit() {
            return Some(digit.to_string());
        }
    }
    // Enter and Return share a value; the token table names the canonical one.
    let canonical = WPF_KEYS
        .iter()
        .filter(|(_, candidate)| *candidate == value)
        .find_map(|(name, _)| {
            BINDING_TOKENS
                .iter()
                .find(|(wpf, _)| wpf == name)
                .map(|(_, token)| *token)
        });
    Some(canonical.unwrap_or(name).to_string())
}

/// The `(Key, ModifierKeys)` integers of a binding string such as
/// `"Ctrl+Shift+F5"`. A binding with no key (or a key WPF has no member for)
/// has no upstream representation and is written as `Key.None`.
pub(super) fn binding_to_wpf(binding: &str) -> (i32, i32) {
    let mut modifiers = 0;
    let mut key = 0;
    for token in binding.split('+').map(str::trim).filter(|t| !t.is_empty()) {
        match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= MODIFIER_CONTROL,
            "alt" | "meta" => modifiers |= MODIFIER_ALT,
            "shift" => modifiers |= MODIFIER_SHIFT,
            "super" | "win" | "windows" => modifiers |= MODIFIER_WINDOWS,
            _ => key = key_for_token(token).unwrap_or(0),
        }
    }
    (key, modifiers)
}

/// The binding string for the `(Key, ModifierKeys)` integers of an upstream
/// `KeyBindingConfig`. A key value outside the `Key` enum reads as `Key.None`.
pub(super) fn wpf_to_binding(key: i32, modifiers: i32) -> String {
    let mut parts = Vec::new();
    for (bit, name) in [
        (MODIFIER_CONTROL, "Ctrl"),
        (MODIFIER_ALT, "Alt"),
        (MODIFIER_SHIFT, "Shift"),
        (MODIFIER_WINDOWS, "Super"),
    ] {
        if modifiers & bit != 0 {
            parts.push(name.to_string());
        }
    }
    parts.push(token_for_key(key).unwrap_or_else(|| "None".to_string()));
    parts.join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_table_matches_the_wpf_enum_values() {
        assert_eq!(key_value("None"), Some(0));
        assert_eq!(key_value("Back"), Some(2));
        assert_eq!(key_value("Enter"), key_value("Return"));
        assert_eq!(key_value("Space"), Some(18));
        assert_eq!(key_value("D0"), Some(34));
        assert_eq!(key_value("A"), Some(44));
        assert_eq!(key_value("F1"), Some(90));
        assert_eq!(key_value("F5"), Some(94));
        assert_eq!(key_value("F12"), Some(101));
        assert_eq!(key_value("DeadCharProcessed"), Some(172));
    }

    #[test]
    fn upstream_defaults_map_to_their_integers() {
        // SettingsD4.cs:99-153: Control (2) plus F5/F6/F10/F9/F7/F12/F11.
        for (binding, key) in [
            ("Ctrl+F5", 94),
            ("Ctrl+F6", 95),
            ("Ctrl+F10", 99),
            ("Ctrl+F9", 98),
            ("Ctrl+F7", 96),
            ("Ctrl+F12", 101),
            ("Ctrl+F11", 100),
        ] {
            assert_eq!(binding_to_wpf(binding), (key, 2), "{binding}");
            assert_eq!(wpf_to_binding(key, 2), binding);
        }
    }

    #[test]
    fn modifiers_combine_as_flags() {
        assert_eq!(binding_to_wpf("Ctrl+Alt+Shift+Super+A"), (44, 15));
        assert_eq!(wpf_to_binding(44, 15), "Ctrl+Alt+Shift+Super+A");
        assert_eq!(binding_to_wpf("F5"), (94, 0));
        assert_eq!(wpf_to_binding(94, 0), "F5");
    }

    #[test]
    fn d4lc_key_tokens_round_trip() {
        for token in [
            "A",
            "Z",
            "0",
            "9",
            "F1",
            "F12",
            "Tab",
            "Enter",
            "Space",
            "Escape",
            "Backspace",
            "Insert",
            "Delete",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Up",
            "Down",
            "Left",
            "Right",
            "Minus",
            "Equal",
            "Comma",
            "Period",
            "Slash",
            "Backslash",
            "Semicolon",
            "Apostrophe",
            "BracketLeft",
            "BracketRight",
            "Grave",
        ] {
            let binding = format!("Ctrl+{token}");
            let (key, modifiers) = binding_to_wpf(&binding);
            assert_ne!(key, 0, "{token} has a WPF key");
            assert_eq!(wpf_to_binding(key, modifiers), binding, "{token}");
        }
    }

    #[test]
    fn binding_aliases_and_wpf_names_resolve() {
        assert_eq!(binding_to_wpf("Ctrl+Esc"), binding_to_wpf("Ctrl+Escape"));
        assert_eq!(binding_to_wpf("Ctrl+-"), binding_to_wpf("Ctrl+Minus"));
        assert_eq!(binding_to_wpf("Alt+f5"), (94, 1));
        // A WPF name d4lc has no token for keeps its WPF name.
        let (key, modifiers) = binding_to_wpf("Ctrl+NumPad5");
        assert_eq!(wpf_to_binding(key, modifiers), "Ctrl+NumPad5");
    }

    #[test]
    fn unmappable_input_reads_as_key_none() {
        assert_eq!(binding_to_wpf("Ctrl+Nonsense"), (0, 2));
        assert_eq!(wpf_to_binding(0, 2), "Ctrl+None");
        assert_eq!(wpf_to_binding(9999, 0), "None");
    }
}
