//! D4Builds: full DOM scrape + conversion download.
//!
//! Upstream `DownloadD4BuildsBuild` (BuildsManagerD4Builds.cs:281-395) is
//! pure DOM work: navigate, wait for the `renameBuild` marker (WebDriverWait
//! 10 s) plus a hard 5 000 ms render sleep (the port: a bounded readiness
//! check, see below), open the variant dropdown and
//! walk its options, and per option walk the `builder__navigation__link`
//! tabs (gear 0; paragon 2 only when `IsImportParagonD4BuildsEnabled`)
//! reading the raw site texts. The port splits that flow at the same seam
//! the crate already uses for the conversion: the browser walk lives here
//! and produces a `d4lc_rules::D4BuildsSiteBuild`; the pure cleanup /
//! fuzzy-matching / paragon-tile conversion lives in d4lc-rules and is
//! offline-testable from a recorded extraction fixture.
//!
//! Timing: the 100 ms `_delayVariant` / `_delayTab` sleeps run verbatim
//! (cancellable) on the worker thread. The page load waits for the load
//! event, then polls the build-name marker under upstream's 10 s
//! WebDriverWait bound. Upstream's fixed 5 000 ms render sleep is replaced
//! by a readiness check (name, gear and stat rows present and unchanged
//! across two polls) that keeps the 5 s as its upper bound, so the flow
//! never waits longer than upstream and never hangs.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use d4lc_rules::ensure_cache_filename_safe;

use super::{bot_protection_failure, BrowserSession, ImportError, ImportErrorKind};

/// Upstream inter-step delays (`_delayVariant` / `_delayTab`,
/// BuildsManagerD4Builds.cs:29-30).
const D4BUILDS_STEP_DELAY: Duration = Duration::from_millis(100);

/// Upstream hard sleep after the page load (`BuildsManagerD4Builds.cs:316`).
/// The port uses it as the upper bound of the render readiness poll.
const D4BUILDS_RENDER_SETTLE_MAX: Duration = Duration::from_millis(5000);

/// Upstream WebDriverWait timeout (`InitSelenium`,
/// BuildsManagerD4Builds.cs:264): bounds the legacy-page marker poll.
const D4BUILDS_ELEMENT_WAIT: Duration = Duration::from_secs(10);

/// Marker-poll cadence (the port's bounded equivalent of upstream's
/// `WebDriverWait.Until` re-probing).
const D4BUILDS_POLL_STEP: Duration = Duration::from_millis(100);

/// Outcome of a successful D4Builds download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct D4BuildsDownloadOutcome {
    pub path: PathBuf,
    pub build_id: String,
    pub build_name: String,
    pub variant_names: Vec<String>,
}

/// Status sink of the D4Builds flow: `(status, build name, variant names
/// so far)` — the same shape the import worker pushes into the event
/// buffer, mirroring the upstream `D4BuildsStatusUpdateMessage` payloads.
pub type D4BuildsStatusSink<'a> = dyn Fn(&str, &str, Vec<String>) + 'a;

/// The build-name probe upstream polls: `renameBuild` value on the legacy
/// page, `builder__header__description` innerHTML otherwise
/// (`BuildsManagerD4Builds.cs:306,319-326`). Always yields a string.
const D4BUILDS_NAME_SCRIPT: &str = "String((document.getElementById('renameBuild') || {}).value \
     || (document.querySelector('.builder__header__description') || {}).innerHTML || '')";

/// Render readiness probe: the counts the extraction reads. Always yields
/// a JSON string.
const D4BUILDS_READY_SCRIPT: &str = "JSON.stringify([\
     document.querySelectorAll('.builder__gear__name').length, \
     document.querySelectorAll('.builder__stat .filled').length, \
     document.querySelectorAll('.builder__gem__slot').length, \
     document.querySelector('.item__arrow__icon--variant') ? 1 : 0])";

/// The last-updated text (`GetLastUpdateInfo`,
/// BuildsManagerD4Builds.cs:915-925).
const D4BUILDS_LAST_UPDATED_SCRIPT: &str =
    "String((document.querySelector('.builder__last__updated') || {}).innerText || '')";

/// The variant dropdown toggle upstream clicks through `ExecuteScript`
/// (`ExportBuildVariants`, BuildsManagerD4Builds.cs:612-614). Returns
/// `ok` / `missing`.
const D4BUILDS_VARIANT_ARROW_SCRIPT: &str = "String((function () { var e = \
     document.querySelector('.item__arrow__icon--variant'); if (!e) { return 'missing'; } \
     e.click(); return 'ok'; })())";

/// The dropdown option labels (`ExportBuildVariants`,
/// BuildsManagerD4Builds.cs:619).
const D4BUILDS_VARIANT_OPTIONS_SCRIPT: &str = "JSON.stringify(Array.prototype.map.call(\
     document.querySelectorAll('.dropdown__option'), function (e) { return e.innerHTML; }))";

/// The gear-tab extraction: aspects / uniques from `builder__gear__name`,
/// runes from `builder__gem__slot`, and the per-slot stat groups
/// (`GetAllAffixes` / `GetAllAspects` / `GetAllRunes` / `GetAllUniques`,
/// BuildsManagerD4Builds.cs:716-820). Missing groups yield empty lists
/// like upstream's per-group catch.
const D4BUILDS_GEAR_SCRIPT: &str = r#"JSON.stringify((function () {
    var gear = Array.prototype.map.call(document.querySelectorAll('.builder__gear__name'), function (e) { return String(e.innerHTML || ''); });
    var slots = ['Helm', 'ChestArmor', 'Gloves', 'Pants', 'Boots', 'Amulet', 'Ring1', 'Ring2', 'Weapon', 'BludgeoningWeapon', 'SlashingWeapon', 'Dual-WieldWeapon1', 'Dual-WieldWeapon2', 'RangedWeapon', 'Offhand'];
    var groups = {};
    slots.forEach(function (key) {
        try {
            var g = document.querySelector('.builder__stats__group.' + key);
            var stats = g ? g.querySelectorAll('.builder__stat') : [];
            var list = [];
            Array.prototype.forEach.call(stats, function (s) {
                var filled = s.querySelector('.filled');
                if (!filled) { return; }
                var html = s.outerHTML;
                list.push({ text: String(filled.innerText || ''), is_greater: html.indexOf('greater__affix__button--filled') !== -1, is_implicit: html.indexOf('implicit') !== -1, is_tempered: html.indexOf('tempering') !== -1 });
            });
            groups[key] = list;
        } catch (err) { groups[key] = []; }
    });
    return {
        aspects: gear.filter(function (t) { return t.indexOf('Aspect') !== -1; }),
        uniques: gear.filter(function (t) { return t.indexOf('Aspect') === -1; }),
        runes: Array.prototype.map.call(document.querySelectorAll('.builder__gem__slot'), function (e) { return String(e.innerHTML || ''); }).filter(function (t) { return t.length === 3 || t.length === 4; }),
        groups: groups
    };
})())"#;

/// The paragon-tab extraction: board name / glyph innerText, the raw
/// `style` attribute, and every tile `class` attribute
/// (`GetAllParagonBoards`, BuildsManagerD4Builds.cs:822-913). A board
/// without `paragon__board__name` reports `null` and fails the flow like
/// upstream's throwing `FindElement`.
const D4BUILDS_PARAGON_SCRIPT: &str = r#"JSON.stringify(Array.prototype.map.call(document.querySelectorAll('.paragon__board'), function (b) {
    var name = b.querySelector('.paragon__board__name');
    var glyphs = b.querySelectorAll('.paragon__board__name__glyph');
    return {
        name: name ? String(name.innerText || '') : null,
        glyph: glyphs.length ? String(glyphs[0].innerText || '') : '',
        style: String(b.getAttribute('style') || ''),
        tiles: Array.prototype.map.call(b.querySelectorAll('.paragon__board__tile'), function (t) { return String(t.getAttribute('class') || ''); })
    };
}))"#;

/// The upstream slot keys of `GetAllAffixes`, in the order the entity
/// lists are assembled (`ExportBuildVariant`,
/// BuildsManagerD4Builds.cs:669-689).
const D4BUILDS_SLOT_KEYS: [&str; 15] = [
    "Helm",
    "ChestArmor",
    "Gloves",
    "Pants",
    "Boots",
    "Amulet",
    "Ring1",
    "Ring2",
    "Weapon",
    "BludgeoningWeapon",
    "SlashingWeapon",
    "Dual-WieldWeapon1",
    "Dual-WieldWeapon2",
    "RangedWeapon",
    "Offhand",
];

/// Click a dropdown option by index (`ExportBuildVariants`,
/// BuildsManagerD4Builds.cs:625). Returns `ok` / `missing`.
fn d4builds_option_click_script(index: usize) -> String {
    format!(
        "String((function () {{ var e = document.querySelectorAll('.dropdown__option')[{index}]; \
         if (!e) {{ return 'missing'; }} e.click(); return 'ok'; }})())"
    )
}

/// Click a `builder__navigation__link` tab (`ExportBuildVariant`,
/// BuildsManagerD4Builds.cs:661,697). Returns the tab count / `missing`.
fn d4builds_tab_click_script(index: usize) -> String {
    format!(
        "String((function () {{ var t = document.querySelectorAll('.builder__navigation__link'); \
         if (!t[{index}]) {{ return 'missing'; }} t[{index}].click(); return String(t.length); \
         }})())"
    )
}

/// Raw extraction payload of [`D4BUILDS_GEAR_SCRIPT`].
#[derive(serde::Deserialize)]
struct D4BuildsGearExtraction {
    aspects: Vec<String>,
    uniques: Vec<String>,
    runes: Vec<String>,
    groups: std::collections::HashMap<String, Vec<D4BuildsSiteAffixExtraction>>,
}

/// Raw values of one `builder__stat` element.
#[derive(serde::Deserialize)]
struct D4BuildsSiteAffixExtraction {
    text: String,
    #[serde(default)]
    is_greater: bool,
    #[serde(default)]
    is_implicit: bool,
    #[serde(default)]
    is_tempered: bool,
}

/// Raw extraction payload of [`D4BUILDS_PARAGON_SCRIPT`]; `name` is `None`
/// when the board carries no `paragon__board__name`.
#[derive(serde::Deserialize)]
struct D4BuildsParagonExtraction {
    name: Option<String>,
    #[serde(default)]
    glyph: String,
    #[serde(default)]
    style: String,
    #[serde(default)]
    tiles: Vec<String>,
}

fn d4builds_evaluate(session: &dyn BrowserSession, script: &str) -> Result<String, ImportError> {
    session
        .evaluate(script)
        .map_err(|error| ImportError::from(error).context("D4Builds page script"))
}

/// A page element the extraction needs is missing: the page does not have
/// the expected build layout.
fn d4builds_layout_error(message: String) -> ImportError {
    ImportError::new(ImportErrorKind::InvalidResponse, message)
}

/// Evaluate a click script and reject the `missing` answer (upstream
/// `FindElement` throws and fails the download).
fn d4builds_click(
    session: &dyn BrowserSession,
    script: &str,
    what: &str,
) -> Result<(), ImportError> {
    let answer = d4builds_evaluate(session, script)?;
    if answer == "missing" {
        Err(d4builds_layout_error(format!(
            "{what} element not found on the D4Builds page"
        )))
    } else {
        Ok(())
    }
}

/// Wait until the build content finished rendering: gear names, stat rows
/// and the variant arrow are present and unchanged across two polls. The
/// wait ends at [`D4BUILDS_RENDER_SETTLE_MAX`] (upstream's fixed sleep)
/// even when the page never settles; the extraction then reports what is
/// missing.
fn d4builds_wait_rendered(session: &dyn BrowserSession) -> Result<(), ImportError> {
    let deadline = Instant::now() + D4BUILDS_RENDER_SETTLE_MAX;
    let mut previous: Option<String> = None;
    loop {
        let counts = d4builds_evaluate(session, D4BUILDS_READY_SCRIPT)?;
        let rendered = serde_json::from_str::<[u32; 4]>(&counts)
            .is_ok_and(|[gear, stats, _gems, arrow]| gear > 0 && stats > 0 && arrow > 0);
        if rendered && previous.as_deref() == Some(counts.as_str()) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        previous = Some(counts);
        session.pause(D4BUILDS_POLL_STEP)?;
    }
}

/// Download a D4Builds build through the injected browser session:
/// scrape every variant's raw site texts, convert them to the stored
/// `D4BuildsBuild` entity with the pure d4lc-rules conversion, and persist
/// it as indented JSON at `{builds_dir}/{id}.json` (upstream
/// `DownloadD4BuildsBuild` + `ConvertBuildVariants` + `WriteIndented`
/// save). Does **not** mutate `AppState`; the caller projects the outcome
/// to the UI.
pub fn download_d4builds_build(
    build_id: &str,
    builds_dir: &Path,
    session: &dyn BrowserSession,
    repo_root: &Path,
    paragon_enabled: bool,
    push_status: &D4BuildsStatusSink<'_>,
) -> Result<D4BuildsDownloadOutcome, ImportError> {
    let site = scrape_d4builds_site_build(build_id, session, paragon_enabled, push_status)?;
    let started = Instant::now();
    let outcome = convert_and_store_d4builds_site(&site, builds_dir, repo_root, push_status)?;
    session.record_phase("convert_store", started.elapsed());
    Ok(outcome)
}

/// Convert a scraped site build to the stored entity and persist it — the
/// pure half of [`download_d4builds_build`], split out so the live smoke
/// can dump the raw extraction before converting. Emits the upstream
/// `Converting {v}.` / `Converted {v}.` statuses.
pub fn convert_and_store_d4builds_site(
    site: &d4lc_rules::D4BuildsSiteBuild,
    builds_dir: &Path,
    repo_root: &Path,
    push_status: &D4BuildsStatusSink<'_>,
) -> Result<D4BuildsDownloadOutcome, ImportError> {
    let build_id = &site.build_id;
    let repo = d4lc_assets::AssetRepository::new(repo_root);
    let lookups = d4lc_rules::D4BuildsConversionLookups::load(&repo).map_err(|error| {
        ImportError::new(
            ImportErrorKind::Parse,
            format!("D4Builds conversion data unavailable for '{build_id}': {error}"),
        )
    })?;

    let mut warning_count = 0usize;
    let mut variants = Vec::with_capacity(site.variants.len());
    for site_variant in &site.variants {
        push_status(
            &format!("Converting {}.", site_variant.name),
            &site.name,
            Vec::new(),
        );
        let converted = lookups.convert_variant(site_variant).map_err(|error| {
            ImportError::new(
                ImportErrorKind::Parse,
                format!("D4Builds conversion failed for '{build_id}': {error}"),
            )
        })?;
        warning_count += converted.warnings.len();
        variants.push(converted.variant);
        push_status(
            &format!("Converted {}.", site_variant.name),
            &site.name,
            Vec::new(),
        );
    }
    if warning_count > 0 {
        // Upstream reports nothing for site texts that do not map, so this
        // stays a diagnostic on stderr only.
        eprintln!(
            "[d4lc] d4builds download '{build_id}': {warning_count} site text(s) did not map to \
             canonical ids"
        );
    }

    let variant_names = variants.iter().map(|v| v.name.clone()).collect();
    let entity = d4lc_rules::D4BuildsBuildFile {
        date: site.date.clone(),
        id: site.build_id.clone(),
        name: site.name.clone(),
        variants,
    };
    let path = d4lc_rules::export_d4builds_build_cache(&entity, builds_dir)
        .map_err(ImportError::of(ImportErrorKind::Storage))?;
    Ok(D4BuildsDownloadOutcome {
        path,
        build_id: site.build_id.clone(),
        build_name: site.name.clone(),
        variant_names,
    })
}

/// Scrape one d4builds.gg build page into the raw site texts (upstream
/// `DownloadD4BuildsBuild` + `ExportBuildVariants` browser walk). The
/// upstream status sequence is emitted through `push_status` verbatim:
/// `Downloading {id}.` / `Downloaded {name}.` / `Exporting {v}.` /
/// `Paragon: {name} ({glyph}).` / `Exported {v}.`
pub fn scrape_d4builds_site_build(
    build_id: &str,
    session: &dyn BrowserSession,
    paragon_enabled: bool,
    push_status: &D4BuildsStatusSink<'_>,
) -> Result<d4lc_rules::D4BuildsSiteBuild, ImportError> {
    ensure_cache_filename_safe(build_id).map_err(ImportError::of(ImportErrorKind::InvalidUrl))?;
    let page_url = build_d4builds_page_url(build_id);
    // Upstream status: `$"Downloading {d4BuildsBuild.Id}."` (:296-300).
    push_status(&format!("Downloading {build_id}."), "", Vec::new());

    // Upstream navigates and then waits for the legacy marker. The port
    // waits for the document's load event and then polls the same marker
    // with upstream's 10 s WebDriverWait bound.
    session.navigate(&page_url).map_err(|error| {
        ImportError::from(error).context(format!("D4Builds page load for '{build_id}'"))
    })?;
    let started = Instant::now();
    let deadline = started + D4BUILDS_ELEMENT_WAIT;
    let mut name = String::new();
    while name.is_empty() {
        if Instant::now() >= deadline {
            return Err(
                match bot_protection_failure(session, "D4Builds", &page_url, "Try again later.") {
                    Some(message) => ImportError::new(ImportErrorKind::Blocked, message),
                    None => ImportError::new(
                        ImportErrorKind::Timeout,
                        format!(
                            "the D4Builds page for '{build_id}' showed no build within {} s \
                             (upstream renameBuild wait)",
                            D4BUILDS_ELEMENT_WAIT.as_secs()
                        ),
                    ),
                },
            );
        }
        name = d4builds_evaluate(session, D4BUILDS_NAME_SCRIPT)?;
        if name.is_empty() {
            session.pause(D4BUILDS_POLL_STEP)?;
        }
    }
    // Upstream sleeps 5 s after the marker (:314-316); the port waits for
    // the rendered content instead, bounded by the same 5 s.
    d4builds_wait_rendered(session)?;
    session.record_phase("data_wait", started.elapsed());
    let started = Instant::now();
    push_status(&format!("Downloaded {name}."), &name, Vec::new());

    // Last update (`GetLastUpdateInfo`): empty text falls back to the
    // current timestamp like upstream's catch-branch `DateTime.Now` (UTC
    // rendering, no timezone database).
    let date = match d4builds_evaluate(session, D4BUILDS_LAST_UPDATED_SCRIPT)? {
        text if !text.is_empty() => text,
        _ => d4lc_rules::unix_ms_to_utc_timestamp(
            i64::try_from(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|duration| duration.as_millis())
                    .unwrap_or(0),
            )
            .unwrap_or(0),
        ),
    };

    let mut site = d4lc_rules::D4BuildsSiteBuild {
        build_id: build_id.to_string(),
        name,
        date,
        variants: Vec::new(),
    };

    // Open the variant dropdown (upstream clicks the arrow through
    // ExecuteScript, :612-614).
    d4builds_click(
        session,
        D4BUILDS_VARIANT_ARROW_SCRIPT,
        "the variant dropdown arrow",
    )?;
    session.pause(D4BUILDS_STEP_DELAY)?;
    let mut options = d4builds_read_variant_options(session)?;
    let variant_count = options.len();

    for index in 0..variant_count {
        let variant_name = options.get(index).cloned().ok_or_else(|| {
            d4builds_layout_error(format!(
                "the variant dropdown shrank below {index} options while exporting '{build_id}'"
            ))
        })?;
        d4builds_click(
            session,
            &d4builds_option_click_script(index),
            &format!("variant option {index}"),
        )?;
        session.pause(D4BUILDS_STEP_DELAY)?;

        push_status(
            &format!("Exporting {variant_name}."),
            &site.name,
            site.variants
                .iter()
                .map(|variant| variant.name.clone())
                .collect(),
        );
        let mut site_variant = d4lc_rules::D4BuildsSiteVariant {
            name: variant_name.clone(),
            ..d4lc_rules::D4BuildsSiteVariant::default()
        };

        // "Gear & Skills" tab (upstream tab index 0, :660-662). No tabs at
        // all drops the variant silently, exactly like upstream's
        // `count > 0` guard.
        let tabs = d4builds_evaluate(session, &d4builds_tab_click_script(0))?;
        if tabs != "missing" {
            session.pause(D4BUILDS_STEP_DELAY)?;
            let gear: D4BuildsGearExtraction = serde_json::from_str(&d4builds_evaluate(
                session,
                D4BUILDS_GEAR_SCRIPT,
            )?)
            .map_err(|error| {
                d4builds_layout_error(format!("gear extraction was not valid JSON: {error}"))
            })?;
            site_variant.aspects = gear.aspects;
            site_variant.uniques = gear.uniques;
            site_variant.runes = gear.runes;
            site_variant.affix_groups = D4BUILDS_SLOT_KEYS
                .iter()
                .filter_map(|slot| {
                    gear.groups
                        .get(*slot)
                        .map(|affixes| d4lc_rules::D4BuildsSiteAffixGroup {
                            slot: (*slot).to_string(),
                            affixes: affixes
                                .iter()
                                .map(|affix| d4lc_rules::D4BuildsSiteAffix {
                                    text: affix.text.clone(),
                                    is_greater: affix.is_greater,
                                    is_implicit: affix.is_implicit,
                                    is_tempered: affix.is_tempered,
                                })
                                .collect(),
                        })
                })
                .filter(|group| !group.affixes.is_empty())
                .collect();
        }

        // Paragon tab (upstream tab index 2, gated on the import
        // setting, :694-702). A missing tab index throws upstream and
        // fails the download.
        if paragon_enabled {
            let paragon_tabs = d4builds_evaluate(session, &d4builds_tab_click_script(2))?;
            if paragon_tabs == "missing" {
                return Err(d4builds_layout_error(format!(
                    "D4Builds page for '{build_id}' has no paragon tab (upstream \
                     builder__navigation__link[2])"
                )));
            }
            session.pause(D4BUILDS_STEP_DELAY)?;
            let boards: Vec<D4BuildsParagonExtraction> =
                serde_json::from_str(&d4builds_evaluate(session, D4BUILDS_PARAGON_SCRIPT)?)
                    .map_err(|error| {
                        d4builds_layout_error(format!(
                            "paragon extraction was not valid JSON: {error}"
                        ))
                    })?;
            site_variant.paragon_boards = boards
                .into_iter()
                .map(|board| {
                    Ok(d4lc_rules::D4BuildsSiteParagonBoard {
                        name: board.name.ok_or_else(|| {
                            d4builds_layout_error(
                                "a paragon board carries no paragon__board__name element"
                                    .to_string(),
                            )
                        })?,
                        glyph: board.glyph,
                        style: board.style,
                        tiles: board.tiles,
                    })
                })
                .collect::<Result<Vec<_>, ImportError>>()?;
            // Upstream emits the per-board status while exporting
            // (:844-848); the pure parse gives the processed name / glyph.
            let parsed =
                d4lc_rules::parse_d4builds_site_paragon_boards(&site_variant.paragon_boards)
                    .map_err(|error| {
                        ImportError::new(
                            ImportErrorKind::Parse,
                            format!("D4Builds paragon scrape failed for '{build_id}': {error}"),
                        )
                    })?;
            for board in &parsed {
                push_status(
                    &format!("Paragon: {} ({}).", board.name, board.glyph),
                    &site.name,
                    site.variants
                        .iter()
                        .map(|variant| variant.name.clone())
                        .collect(),
                );
            }
        }

        site.variants.push(site_variant);
        let exported: Vec<String> = site
            .variants
            .iter()
            .map(|variant| variant.name.clone())
            .collect();
        push_status(&format!("Exported {variant_name}."), &site.name, exported);

        // Open the dropdown menu again for the next variant (:630-633).
        d4builds_click(
            session,
            D4BUILDS_VARIANT_ARROW_SCRIPT,
            "the variant dropdown arrow",
        )?;
        session.pause(D4BUILDS_STEP_DELAY)?;
        options = d4builds_read_variant_options(session)?;
    }
    if site.variants.is_empty() {
        return Err(d4builds_layout_error(format!(
            "the D4Builds page for '{build_id}' lists no build variants"
        )));
    }
    session.record_phase("extract", started.elapsed());

    Ok(site)
}

fn d4builds_read_variant_options(session: &dyn BrowserSession) -> Result<Vec<String>, ImportError> {
    serde_json::from_str(&d4builds_evaluate(
        session,
        D4BUILDS_VARIANT_OPTIONS_SCRIPT,
    )?)
    .map_err(|error| d4builds_layout_error(format!("variant options were not valid JSON: {error}")))
}

/// The page upstream navigates (`BuildsManagerD4Builds.cs:302`).
pub fn build_d4builds_page_url(build_id: &str) -> String {
    format!("https://d4builds.gg/builds/{build_id}/?var=0")
}
