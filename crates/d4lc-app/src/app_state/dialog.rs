use d4lc_gui::dialog::ConfirmAction;

// ---------------------------------------------------------------------------
// DialogKind — stable identifier for every dialog reachable from the shared
// `DialogHost`. One variant per `crates/d4lc-app/ui/dialogs/*.slint`
// component; the only stable reference for dialog routing.
// ---------------------------------------------------------------------------

/// All dialog kinds reachable from the shared `DialogHost`.
///
/// A new dialog component must add a variant here and a dispatch branch in
/// `main_window.slint`'s dialog host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DialogKind {
    // Settings family.
    ColorsConfig,
    ControllerConfig,
    HotkeysConfig,
    OverlayConfig,
    ParagonConfig,
    // Affix family.
    AffixConfig,
    AspectConfig,
    SigilConfig,
    UniqueConfig,
    RuneConfig,
    MultiBuildConfig,
    SetAffix,
    SetAffixColor,
    SetPresetName,
    RenamePreset,
    ImportPreset,
    // Trade family.
    AddTradeItem,
    TradeConfig,
    // Hotkeys family.
    HotkeyConfig,
    // Confirm-prompt family: the one generic window-modal
    // Yes/No prompt every destructive action routes through.
    Confirm,
}

impl DialogKind {
    /// Stable string identifier used as the `dialog-id` value in
    /// `main_window.slint`. Lower-snake-case to match the WPF
    /// `Views/Dialogs/*` file names.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ColorsConfig => "colors_config",
            Self::ControllerConfig => "controller_config",
            Self::HotkeysConfig => "hotkeys_config",
            Self::OverlayConfig => "overlay_config",
            Self::ParagonConfig => "paragon_config",
            Self::AffixConfig => "affix_config",
            Self::AspectConfig => "aspect_config",
            Self::SigilConfig => "sigil_config",
            Self::UniqueConfig => "unique_config",
            Self::RuneConfig => "rune_config",
            Self::MultiBuildConfig => "multi_build_config",
            Self::SetAffix => "set_affix",
            Self::SetAffixColor => "set_affix_color",
            Self::SetPresetName => "set_preset_name",
            Self::RenamePreset => "rename_preset",
            Self::ImportPreset => "import_preset",
            Self::AddTradeItem => "add_trade_item",
            Self::TradeConfig => "trade_config",
            Self::HotkeyConfig => "hotkey_config",
            Self::Confirm => "confirm",
        }
    }

    /// The title in the current UI language: upstream's resource strings for
    /// the dialogs whose title upstream localizes (`AffixViewModel.cs:1781,
    /// 1809`, `ImportAffixPresetViewModel.cs:806`); the rest are English
    /// literals upstream too ([`Self::default_title`]).
    pub fn title(self) -> String {
        match self {
            Self::ImportPreset => crate::i18n::tr("Import preset"),
            Self::RenamePreset => crate::i18n::tr("Rename"),
            Self::SetPresetName => crate::i18n::tr("Confirm name"),
            other => other.default_title().to_string(),
        }
    }

    /// Default human-readable title for the dialog.
    pub const fn default_title(self) -> &'static str {
        match self {
            Self::ColorsConfig => "Default colors config",
            Self::ControllerConfig => "Controller config",
            Self::HotkeysConfig => "Hotkeys config",
            Self::OverlayConfig => "Overlay config",
            Self::ParagonConfig => "Paragon config",
            Self::AffixConfig => "Affix Config",
            Self::AspectConfig => "Aspect Config",
            Self::SigilConfig => "Sigil Config",
            Self::UniqueConfig => "Unique Config",
            Self::RuneConfig => "Rune Config",
            Self::MultiBuildConfig => "Multi-build config",
            Self::SetAffix => "Set Affix",
            Self::SetAffixColor => "Set Affix Color",
            Self::SetPresetName => "Confirm name",
            Self::RenamePreset => "Rename",
            Self::ImportPreset => "Import preset",
            Self::AddTradeItem => "Add Trade Item",
            Self::TradeConfig => "Trade Configuration",
            Self::HotkeyConfig => "Hotkey Configuration",
            // `dialogs::open_confirm` overrides the title with the pending
            // action's upstream title ("Delete" / "Reset"); this default
            // only covers a prompt opened without one.
            Self::Confirm => "Confirm",
        }
    }

    /// Family grouping used by the shared router to pick a confirm/cancel
    /// implementation.
    pub const fn family(self) -> DialogFamily {
        match self {
            Self::ColorsConfig
            | Self::ControllerConfig
            | Self::HotkeysConfig
            | Self::OverlayConfig
            | Self::ParagonConfig => DialogFamily::Settings,
            Self::AffixConfig
            | Self::AspectConfig
            | Self::SigilConfig
            | Self::UniqueConfig
            | Self::RuneConfig
            | Self::SetAffix
            | Self::SetAffixColor
            | Self::SetPresetName
            | Self::RenamePreset => DialogFamily::Affix,
            Self::AddTradeItem | Self::TradeConfig => DialogFamily::Trade,
            Self::HotkeyConfig => DialogFamily::Hotkeys,
            Self::ImportPreset | Self::MultiBuildConfig => DialogFamily::BuildImport,
            Self::Confirm => DialogFamily::Confirm,
        }
    }

    /// Every dialog kind, in stable order. Used by tests and the
    /// shared dialog router.
    #[cfg(test)]
    pub const ALL: [DialogKind; 20] = [
        Self::ColorsConfig,
        Self::ControllerConfig,
        Self::HotkeysConfig,
        Self::OverlayConfig,
        Self::ParagonConfig,
        Self::AffixConfig,
        Self::AspectConfig,
        Self::SigilConfig,
        Self::UniqueConfig,
        Self::RuneConfig,
        Self::MultiBuildConfig,
        Self::SetAffix,
        Self::SetAffixColor,
        Self::SetPresetName,
        Self::RenamePreset,
        Self::ImportPreset,
        Self::AddTradeItem,
        Self::TradeConfig,
        Self::HotkeyConfig,
        Self::Confirm,
    ];

    /// Parse a string id back into a `DialogKind`. Returns the stable
    /// `None` indicator for unknown ids so callers can report the error
    /// through the shared dialog state instead of panicking.
    #[cfg(test)]
    pub fn from_str_id(s: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.as_str() == s)
    }
}

impl std::fmt::Display for DialogKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Family grouping for the shared dialog router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DialogFamily {
    Settings,
    Affix,
    Trade,
    Hotkeys,
    BuildImport,
    /// The generic confirm prompt (`DialogKind::Confirm`): its confirm body
    /// executes the pending `ConfirmAction` instead of a dialog family's
    /// own form.
    Confirm,
}

// ---------------------------------------------------------------------------
// DialogState — runtime dialog state owned by the desktop app.
// ---------------------------------------------------------------------------

/// State for the currently-open dialog (if any). Owned by `AppState`,
/// projected into `main_window.slint` properties, and driven by the
/// shared `actions::dialogs` router.
///
/// Contract for confirm/cancel/error flow:
///
/// 1. `open(kind, title)` sets `kind`, `title`, clears `error` and
///    `busy`, and flips `dialog-visible` in Slint to `true`.
/// 2. The dialog body in Slint emits `dialog-confirm()` or
///    `dialog-cancel()`. The router reads `state.dialog.kind` and
///    dispatches to the family-specific handler.
/// 3. Family handlers either succeed (router closes the dialog and
///    clears `error`) or fail (router sets `dialog.error` and keeps
///    the dialog open so the user can correct the input).
/// 4. `busy` is set while a long-running operation is in flight
///    (e.g. an async build-import download).
/// 5. `DialogKind::Confirm` is the one generic prompt: the
///    router sets `confirm` to the pending action when it opens the prompt,
///    renders that action's upstream title and message, executes it on Yes
///    and drops it on No. The field is cleared with the closing prompt, so
///    only an open prompt ever carries a pending action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogState {
    /// Currently-open dialog kind. `DialogKind::ColorsConfig` is the
    /// default initial value; it is only meaningful when
    /// `AppState::dialog_visible` is `true`.
    pub kind: DialogKind,
    /// Human-readable title rendered by the shared dialog host.
    pub title: String,
    /// Last validation/error message from a confirm attempt. Empty
    /// when the dialog has no error to display.
    pub error: String,
    /// Set by long-running operations (not yet set by any operation).
    pub busy: bool,
    /// Pending confirm-prompt request, set together with
    /// `kind == DialogKind::Confirm` and `None` for every other kind.
    ///
    /// The router renders the upstream prompt copy from it, executes the
    /// action when the prompt is confirmed and drops it when the prompt is
    /// cancelled, so a prompt can never act on a stale target.
    pub confirm: Option<ConfirmAction>,
    /// The `overlay_enabled` value the open overlay dialog was seeded with.
    /// The live loop can flip the setting while the dialog is open (overlay
    /// menu lock); the confirm only writes the dialog's row when the user
    /// changed it from this seed, so a stale prop cannot revert the flip.
    pub overlay_enabled_seed: Option<bool>,
}

impl Default for DialogState {
    fn default() -> Self {
        Self {
            kind: DialogKind::ColorsConfig,
            title: DialogKind::ColorsConfig.default_title().to_string(),
            error: String::new(),
            busy: false,
            confirm: None,
            overlay_enabled_seed: None,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
