use d4lc_rules::ParagonOverlayState;
#[cfg(feature = "real-runtime")]
use d4lc_rules::RuleOverlayDescriptor;
#[cfg(feature = "real-runtime")]
use std::time::Instant;

/// Runtime-owned selection; UI rules publications must not reset it.
#[derive(Debug, Default)]
pub(super) struct ParagonNavigation {
    route_index: usize,
    board_index: usize,
    board_name: Option<String>,
    #[cfg(feature = "real-runtime")]
    step_hover_started_at: Option<Instant>,
}

impl ParagonNavigation {
    pub(super) fn apply(&self, state: &mut ParagonOverlayState) {
        state.current_route_index = self.route_index;
        state.current_board_index = Some(self.board_index);
        if self.board_name.is_some() {
            state.current_board_name.clone_from(&self.board_name);
        }
    }

    /// Upstream's mouse handling (`HandleMouseUpdatedMessage`,
    /// OverlayHandler.cs:944-974), read off the descriptors so rendering and
    /// interaction cannot disagree about panel geometry:
    ///
    /// - only `X < max(W_b, 100)` reacts, with `W_b` the board panel width
    ///   (the step panel is as wide), truncated to whole pixels;
    /// - below the step panel's bottom edge (`Y - 202 > 0`) the hovered row
    ///   is `(Y - 202) / 52`, so the two pixels between rows belong to the
    ///   row above; an index past the last row leaves the selection alone;
    /// - inside the step panel (`152 < Y < 202`) the step advances every
    ///   500 ms (`_paragonStepTimer`, :87-92, tick :1050-1058);
    /// - anywhere else the step timer stops.
    ///
    /// Returns whether the selection changed.
    #[cfg(feature = "real-runtime")]
    pub(super) fn observe(
        &mut self,
        descriptors: &[RuleOverlayDescriptor],
        cursor: Option<d4lc_core::Point>,
        now: Instant,
    ) -> bool {
        use d4lc_rules::{ParagonOverlayElementKind, RuleOverlayShape, RuleOverlayTarget};
        if let Some(name) = descriptors
            .iter()
            .find_map(|descriptor| match &descriptor.target {
                RuleOverlayTarget::Paragon {
                    kind: ParagonOverlayElementKind::BoardBorder,
                    board_name: Some(name),
                    ..
                } => Some(name),
                _ => None,
            })
        {
            let current = self.board_name.get_or_insert_with(String::new);
            current.clear();
            current.push_str(name);
        }
        let panel = |wanted: ParagonOverlayElementKind| {
            descriptors.iter().find(|descriptor| {
                descriptor.shape == RuleOverlayShape::TextPanel
                    && matches!(
                        &descriptor.target,
                        RuleOverlayTarget::Paragon { kind, .. } if *kind == wanted
                    )
            })
        };
        let (Some(cursor), Some(step)) = (cursor, panel(ParagonOverlayElementKind::StepPanel))
        else {
            self.step_hover_started_at = None;
            return false;
        };
        let step_rect = step.geometry;
        // `(int)panelWidthBoard` (OverlayHandler.cs:636): the truncated width
        // the descriptor carries, not the rounded drawn one.
        let hit_width = match &step.target {
            RuleOverlayTarget::Paragon {
                panel_hit_width: Some(width),
                ..
            } => *width,
            _ => step_rect.width,
        };
        let in_x = i64::from(cursor.x) < i64::from(hit_width).max(100);
        let step_bottom = i64::from(step_rect.y) + i64::from(step_rect.height);
        let cursor_y = i64::from(cursor.y);

        if in_x && cursor_y > i64::from(step_rect.y) && cursor_y < step_bottom {
            let RuleOverlayTarget::Paragon {
                route_index: Some(route_index),
                ..
            } = &step.target
            else {
                return false;
            };
            let start = self.step_hover_started_at.get_or_insert(now);
            if now.duration_since(*start) >= std::time::Duration::from_millis(500) {
                self.route_index = route_index + 1;
                self.step_hover_started_at = Some(now);
                return true;
            }
            return false;
        }
        self.step_hover_started_at = None;

        let below_step = cursor_y - step_bottom;
        if !in_x || below_step <= 0 {
            return false;
        }
        let rows = || {
            descriptors
                .iter()
                .filter_map(|descriptor| match &descriptor.target {
                    RuleOverlayTarget::Paragon {
                        kind: ParagonOverlayElementKind::BoardEntry,
                        board_index: Some(board_index),
                        board_name,
                        ..
                    } if descriptor.shape == RuleOverlayShape::TextPanel => {
                        Some((*board_index, board_name, descriptor.geometry))
                    }
                    _ => None,
                })
        };
        // The pitch is the step panel's height plus the gap before the first
        // row (50 + 2).
        let Some((_, _, first_row)) = rows().next() else {
            return false;
        };
        let pitch = i64::from(step_rect.height) + (i64::from(first_row.y) - step_bottom);
        if pitch <= 0 {
            return false;
        }
        let hovered = (below_step / pitch) as usize;
        let Some((board_index, board_name, _)) =
            rows().find(|(board_index, _, _)| *board_index == hovered)
        else {
            return false;
        };
        if self.board_index == board_index && self.board_name.as_deref() == board_name.as_deref() {
            return false;
        }
        self.board_index = board_index;
        self.board_name = board_name.as_deref().map(str::to_owned);
        true
    }
}

#[cfg(all(test, feature = "real-runtime"))]
mod tests;
