//! The upstream `CanAdd{Provider}BuildExecute` Add gates and the
//! Mobalytics URL status line.

// ---------------------------------------------------------------------------
// ID gates — the exact upstream `CanAdd{Provider}BuildExecute` predicates
// (`ImportAffixPresetViewModel.cs`). Invalid input never shows a message:
// the Add button stays disabled and the extracted-id box stays empty.
// ---------------------------------------------------------------------------

/// Upstream `CanAddD2CoreBuildExecute` (:640-655): the input must contain
/// `d2core.com` and `bd=`; the id is the value after the (single) `bd=`
/// trimmed at `&` and must be exactly 4 characters.
pub fn can_add_d2core_build(input: &str) -> Option<String> {
    let lowered = input.to_ascii_lowercase();
    if !lowered.contains("d2core.com") || !lowered.contains("bd=") {
        return None;
    }
    // Upstream splits with RemoveEmptyEntries and only accepts exactly two
    // parts (a second `bd=` occurrence invalidates the input).
    let parts: Vec<&str> = input.split("bd=").filter(|part| !part.is_empty()).collect();
    if parts.len() != 2 {
        return None;
    }
    let build_id = parts[1].split('&').next().unwrap_or_default().trim();
    (!build_id.is_empty() && build_id.len() == 4).then(|| build_id.to_string())
}

/// Upstream `CanAddD4BuildsBuildExecute` (:677-690): reject
/// `mobalytics.gg`; the longest `/` segment (empty segments removed,
/// first-maximum like C# `MaxBy`), then the longest `?` part, is valid
/// when 36 characters long or containing a `-`.
pub fn can_add_d4builds_build(input: &str) -> Option<String> {
    if input.to_ascii_lowercase().contains("mobalytics.gg") {
        return None;
    }
    let container = longest_segment(input.split('/').filter(|segment| !segment.is_empty()))?;
    let build_id = longest_segment(container.split('?'))?;
    let build_id = build_id.trim();
    (!build_id.is_empty() && (build_id.len() == 36 || build_id.contains('-')))
        .then(|| build_id.to_string())
}

/// First-longest segment (C# `MaxBy` returns the first maximum; Rust's
/// `max_by_key` the last — the helper pins the upstream tie-break).
fn longest_segment<'a>(segments: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    let mut best: Option<&'a str> = None;
    for segment in segments {
        let better = match best {
            None => true,
            Some(current) => segment.len() > current.len(),
        };
        if better {
            best = Some(segment);
        }
    }
    best
}

/// Upstream `CanAddInfinityBuildsBuildExecute` (:712-715): the share URL
/// must contain `infinitybuilds.gg`; the full URL is used.
pub fn can_add_infinity_builds_build(url: &str) -> Option<String> {
    let url = url.trim();
    (!url.is_empty() && url.to_ascii_lowercase().contains("infinitybuilds.gg"))
        .then(|| url.to_string())
}

/// Upstream `CanAddMaxrollBuildExecute` (:737-747): the last non-empty
/// `/` segment trimmed at `#` must be exactly 8 characters.
pub fn can_add_maxroll_build(input: &str) -> Option<String> {
    let build_id = input.rsplit('/').find(|part| !part.is_empty())?;
    let build_id = build_id.split('#').next().unwrap_or_default().trim();
    (!build_id.is_empty() && build_id.len() == 8 && !build_id.contains('#'))
        .then(|| build_id.to_string())
}

/// Upstream `BuildIdMobalytics` setter (:325-340): when the URL addresses
/// a profile's builds page the `?query…` suffix is stripped.
pub fn mobalytics_effective_url(url: &str) -> String {
    let lowered = url.to_ascii_lowercase();
    if lowered.contains("profile") && lowered.contains("builds") {
        url.split('?').next().unwrap_or(url).to_string()
    } else {
        url.to_string()
    }
}

/// Upstream `CanAddMobalyticsBuildExecute` (:754-760): non-empty, starts
/// with `https://mobalytics.gg`, and addresses a profile page, a builds
/// page, or both. The port also requires the host to end there (`/`, `?`,
/// `#` or the end follows): upstream's bare prefix test passes
/// `https://mobalytics.gg.example.com/…` and `https://mobalytics.gg@…`,
/// which the browser would load from another host.
pub fn can_add_mobalytics_build(url: &str) -> Option<String> {
    let url = url.trim();
    let after_host = url.strip_prefix("https://mobalytics.gg")?;
    if !(after_host.is_empty() || after_host.starts_with(['/', '?', '#'])) {
        return None;
    }
    let has_profile = url.contains("profile");
    let has_builds = url.contains("builds");
    if (has_profile && !has_builds) || (!has_profile && has_builds) || (has_profile && has_builds) {
        Some(mobalytics_effective_url(url))
    } else {
        None
    }
}

/// The Mobalytics status-line copy in English
/// (`UrlValidStatusMobalytics`, :602-632; resx
/// `rsCapBuildUrlDetected` / `rsCapProfileUrlDetected` /
/// `rsCapBuildProfileUrlMissing`). The status line itself is translated.
pub const MOBALYTICS_STATUS_BUILD_URL: &str = "Build url detected.";
pub const MOBALYTICS_STATUS_PROFILE_URL: &str = "Profile url detected.";
pub const MOBALYTICS_STATUS_INVALID: &str = "Error: Invalid url. Build or profile id missing.";

/// The persistent Mobalytics URL status line: build/profile detection is
/// case-insensitive (upstream `OrdinalIgnoreCase`), and an empty or
/// non-Mobalytics input shows the error line exactly like upstream.
pub fn mobalytics_url_status(url: &str) -> String {
    let lowered = url.to_ascii_lowercase();
    let has_host = lowered.contains("mobalytics.gg");
    let has_profile = lowered.contains("profile");
    let has_builds = lowered.contains("builds");
    if has_host && has_builds {
        // A builds page (with or without a profile segment) is a build URL
        // like upstream.
        crate::i18n::tr(MOBALYTICS_STATUS_BUILD_URL)
    } else if has_host && has_profile {
        crate::i18n::tr(MOBALYTICS_STATUS_PROFILE_URL)
    } else {
        crate::i18n::tr(MOBALYTICS_STATUS_INVALID)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // ID gates (upstream CanAdd{Provider}BuildExecute parity)
    // -----------------------------------------------------------------------

    #[test]
    fn d2core_gate_requires_host_bd_and_four_characters() {
        assert_eq!(
            can_add_d2core_build("https://www.d2core.com/d4/planner?bd=abcd"),
            Some("abcd".to_string())
        );
        assert_eq!(
            can_add_d2core_build("https://www.d2core.com/d4/planner?x=1&bd=ab12&lang=en"),
            Some("ab12".to_string()),
            "extra query parameters are trimmed at the &"
        );
        assert_eq!(can_add_d2core_build("https://d2core.com/x"), None);
        assert_eq!(
            can_add_d2core_build("https://www.d2core.com/d4/planner"),
            None
        );
        assert_eq!(
            can_add_d2core_build("https://www.d2core.com/d4/planner?bd=abc"),
            None,
            "3 characters fail the gate"
        );
        assert_eq!(
            can_add_d2core_build("https://www.d2core.com/d4/planner?bd=abcde"),
            None,
            "5 characters fail the gate"
        );
        assert_eq!(
            can_add_d2core_build("https://www.d2core.com/d4/planner?bd=ab&bd=cd"),
            None,
            "a second bd= occurrence invalidates the input like upstream Split"
        );
    }

    #[test]
    fn d4builds_gate_takes_the_longest_segment() {
        assert_eq!(
            can_add_d4builds_build(
                "https://d4builds.gg/builds/b7e23ec29af2230b9a3e5c4f1d8a6e2c1234/?var=0"
            ),
            Some("b7e23ec29af2230b9a3e5c4f1d8a6e2c1234".to_string())
        );
        assert_eq!(
            can_add_d4builds_build("https://d4builds.gg/builds/whirlwind-endgame"),
            Some("whirlwind-endgame".to_string()),
            "a hyphen slug passes via the contains-'-' branch"
        );
        assert_eq!(
            can_add_d4builds_build("https://d4builds.gg/builds/shortbuildid?var=0"),
            None,
            "a segment failing both branches fails the gate"
        );
        assert_eq!(
            can_add_d4builds_build("https://mobalytics.gg/diablo-4/builds/x-whirlwind"),
            None,
            "mobalytics.gg is rejected"
        );
    }

    #[test]
    fn infinity_gate_requires_the_share_host() {
        assert_eq!(
            can_add_infinity_builds_build("https://infinitybuilds.gg/en/builds/pestilent"),
            Some("https://infinitybuilds.gg/en/builds/pestilent".to_string())
        );
        assert_eq!(
            can_add_infinity_builds_build("https://evil.example/build"),
            None
        );
        assert_eq!(can_add_infinity_builds_build(""), None);
    }

    #[test]
    fn maxroll_gate_takes_the_last_segment_of_eight() {
        assert_eq!(
            can_add_maxroll_build("https://maxroll.gg/d4/planner/t3jx0916"),
            Some("t3jx0916".to_string())
        );
        assert_eq!(
            can_add_maxroll_build("t3jx0916"),
            Some("t3jx0916".to_string())
        );
        assert_eq!(
            can_add_maxroll_build("https://maxroll.gg/d4/planner/t3jx0916#notes"),
            Some("t3jx0916".to_string()),
            "the #… suffix is trimmed before the length check"
        );
        assert_eq!(can_add_maxroll_build("t3jx091"), None);
        assert_eq!(can_add_maxroll_build("t3jx09166"), None);
        assert_eq!(can_add_maxroll_build(""), None);
    }

    #[test]
    fn mobalytics_gate_accepts_profile_builds_or_both() {
        assert_eq!(
            can_add_mobalytics_build("https://mobalytics.gg/diablo-4/builds/my-build"),
            Some("https://mobalytics.gg/diablo-4/builds/my-build".to_string())
        );
        assert_eq!(
            can_add_mobalytics_build("https://mobalytics.gg/diablo-4/profile/some-user"),
            Some("https://mobalytics.gg/diablo-4/profile/some-user".to_string())
        );
        assert_eq!(
            can_add_mobalytics_build(
                "https://mobalytics.gg/diablo-4/profile/some-user/builds/my-build?query=%7B%7D"
            ),
            Some("https://mobalytics.gg/diablo-4/profile/some-user/builds/my-build".to_string()),
            "the ?query suffix is stripped on profile builds pages"
        );
        assert_eq!(
            can_add_mobalytics_build("https://mobalytics.gg"),
            None,
            "neither profile nor builds fails the gate"
        );
        assert_eq!(
            can_add_mobalytics_build("http://mobalytics.gg/diablo-4/builds/x"),
            None,
            "the https prefix is required"
        );
    }

    #[test]
    fn mobalytics_gate_refuses_lookalike_hosts_and_keeps_every_mobalytics_url() {
        for foreign in [
            "https://mobalytics.gg.evil.example/diablo-4/builds/x",
            "https://mobalytics.gg@evil.example/diablo-4/builds/x",
            "https://mobalytics.gg:8443/diablo-4/builds/x",
            "https://mobalytics.ggx/diablo-4/builds/x",
        ] {
            assert_eq!(can_add_mobalytics_build(foreign), None, "{foreign}");
        }
        for upstream_example in [
            "https://mobalytics.gg/diablo-4/builds/druid-pulverize",
            "https://mobalytics.gg/diablo-4/builds/warlock-dread-claws",
            "https://mobalytics.gg/diablo-4/profile/210623d7-d551-492f-82fc-632f760d8751/builds",
            "https://mobalytics.gg/diablo-4/profile/p4wnyhof/builds/tyrants-grasp-spam-warlock",
            "https://mobalytics.gg/diablo-4/profile/Sanctum",
            "  https://mobalytics.gg/diablo-4/builds/druid-pulverize#variants  ",
        ] {
            assert!(
                can_add_mobalytics_build(upstream_example).is_some(),
                "{upstream_example}"
            );
        }
        assert_eq!(
            can_add_mobalytics_build("https://mobalytics.gg?builds"),
            Some("https://mobalytics.gg?builds".to_string()),
            "a query directly after the host still passes like upstream"
        );
    }

    #[test]
    fn mobalytics_status_line_stays_english_where_upstream_has_no_translation() {
        // None of the shipped upstream resource files translates these keys,
        // so those languages fall back to the neutral text; zh-CN is the one
        // the port translated itself.
        for culture in ["de-DE", "es-ES", "fr-FR"] {
            crate::i18n::with_language(culture, || {
                assert_eq!(
                    mobalytics_url_status("https://mobalytics.gg/diablo-4/builds/x"),
                    MOBALYTICS_STATUS_BUILD_URL,
                    "{culture}"
                );
            });
        }
        crate::i18n::with_language("zh-CN", || {
            assert_eq!(
                mobalytics_url_status("https://mobalytics.gg/diablo-4/builds/x"),
                "检测到Build链接。"
            );
        });
    }

    #[test]
    fn mobalytics_status_line_mirrors_the_three_upstream_states() {
        assert_eq!(
            mobalytics_url_status("https://mobalytics.gg/diablo-4/builds/my-build"),
            MOBALYTICS_STATUS_BUILD_URL
        );
        assert_eq!(
            mobalytics_url_status("https://MOBALYTICS.gg/diablo-4/profile/some-user"),
            MOBALYTICS_STATUS_PROFILE_URL
        );
        assert_eq!(
            mobalytics_url_status(""),
            MOBALYTICS_STATUS_INVALID,
            "an empty input shows the persistent error line like upstream"
        );
        assert_eq!(
            mobalytics_url_status("https://d4builds.gg/builds/x"),
            MOBALYTICS_STATUS_INVALID
        );
    }
}
