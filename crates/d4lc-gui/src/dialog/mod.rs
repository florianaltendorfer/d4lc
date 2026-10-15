//! Headless confirm-prompt state.
//!
//! Upstream asks Yes/No before every destructive action through a
//! window-modal MahApps `ShowMessageAsync` prompt; the port hosts those
//! prompts in one generic `DialogKind::Confirm` dialog. What a prompt asks
//! about lives here, decoupled from Slint and from the app layer, so the one
//! port confirm vocabulary and the upstream copy it mirrors cannot drift
//! apart.
//!
//! ## Upstream evidence
//!
//! - `AffixViewModel.cs:1762-1772` — `RemoveAffixPresetNameExecute` asks
//!   `ShowMessageAsync(this, "Delete", $"Are you sure you want to delete
//!   preset \"{SelectedAffixPreset.Name}\"", MessageDialogStyle.AffirmativeAndNegative)`
//!   and deletes only on `MessageDialogResult.Affirmative`.
//! - `ImportAffixPresetViewModel.cs:1196-1206` — the same prompt from the
//!   import dialog's preset list.
//! - `AffixConfigViewModel.cs:114-123` — `ResetMinimalAffixValuesExecute`
//!   asks `"Reset"` / `"Are you sure you want to reset the minimal affix
//!   values?"` and resets only on `Affirmative`.
//!
//! The Slint prompt renders [`ConfirmAction::title`] and
//! [`ConfirmAction::message`]; the shared dialog router executes the action
//! on Yes and drops it on No.

/// The action a confirm prompt asks about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Delete the named affix preset (the affix screen's remove button and
    /// the import dialog's preset list share one prompt and one copy).
    DeletePreset { name: String },
    /// Clear every stored minimal affix value (affix config dialog).
    ResetMinimalAffixValues,
}

/// The dialog a resolved prompt returns to.
///
/// Upstream's prompt is window-modal and is shown over the dialog that
/// opened it (`ShowMessageAsync`), which stays open beneath it. The port's
/// dialog host renders one dialog at a time, so the router reopens this
/// parent dialog after the prompt is answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmParent {
    /// The affix config dialog (its reset button opens the prompt).
    AffixConfig,
}

impl ConfirmAction {
    /// Upstream prompt title (the `ShowMessageAsync` `title` argument of
    /// the cited lines).
    pub const fn title(&self) -> &'static str {
        match self {
            Self::DeletePreset { .. } => "Delete",
            Self::ResetMinimalAffixValues => "Reset",
        }
    }

    /// Upstream prompt body (the `ShowMessageAsync` `message` argument of
    /// the cited lines), verbatim.
    pub fn message(&self) -> String {
        match self {
            Self::DeletePreset { name } => {
                format!("Are you sure you want to delete preset \"{name}\"")
            }
            Self::ResetMinimalAffixValues => {
                "Are you sure you want to reset the minimal affix values?".to_string()
            }
        }
    }

    /// The dialog the prompt returns to when answered, if any.
    ///
    /// The reset prompt is opened from the affix config dialog; the delete
    /// prompt is opened from the affix screen, which has no dialog of its
    /// own to return to.
    pub const fn parent(&self) -> Option<ConfirmParent> {
        match self {
            Self::DeletePreset { .. } => None,
            Self::ResetMinimalAffixValues => Some(ConfirmParent::AffixConfig),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_preset_prompt_mirrors_upstream_copy() {
        let action = ConfirmAction::DeletePreset {
            name: "Boss".to_string(),
        };
        assert_eq!(action.title(), "Delete");
        assert_eq!(
            action.message(),
            "Are you sure you want to delete preset \"Boss\""
        );
        assert_eq!(action.parent(), None);
    }

    #[test]
    fn reset_prompt_mirrors_upstream_copy() {
        let action = ConfirmAction::ResetMinimalAffixValues;
        assert_eq!(action.title(), "Reset");
        assert_eq!(
            action.message(),
            "Are you sure you want to reset the minimal affix values?"
        );
        assert_eq!(action.parent(), Some(ConfirmParent::AffixConfig));
    }
}
