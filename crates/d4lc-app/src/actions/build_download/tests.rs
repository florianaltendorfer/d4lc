use std::path::PathBuf;

use super::*;

// -----------------------------------------------------------------------
// Fake browser session.
// -----------------------------------------------------------------------

/// Test double standing in for the headless browser: records the
/// navigations (url + filter) and answers with a canned set of JSON
/// responses, a canned evaluate result, or a structured error —
/// exactly what the upstream sniffing flows observe.
struct FakeSession {
    responses: Vec<CapturedResponse>,
    evaluate_result: String,
    /// Answer to the bot-protection probe; empty = probe fails (no page
    /// markup), the state of every non-challenge test.
    probe_result: String,
    /// Successive probe answers; each probe takes the next one and falls
    /// back to `probe_result` once exhausted.
    probe_sequence: std::sync::Mutex<std::collections::VecDeque<String>>,
    /// Whether the launch profile asked for a visible window.
    window_is_shown: bool,
    navigate_error: Option<BrowserError>,
    seen_navigations: std::sync::Mutex<Vec<(String, String)>>,
}

impl FakeSession {
    fn with_responses(responses: Vec<CapturedResponse>) -> Self {
        Self {
            responses,
            evaluate_result: String::new(),
            probe_result: String::new(),
            probe_sequence: std::sync::Mutex::new(std::collections::VecDeque::new()),
            window_is_shown: false,
            navigate_error: None,
            seen_navigations: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn timing_out(url: &str) -> Self {
        Self {
            responses: Vec::new(),
            evaluate_result: String::new(),
            probe_result: String::new(),
            probe_sequence: std::sync::Mutex::new(std::collections::VecDeque::new()),
            window_is_shown: false,
            navigate_error: Some(BrowserError::Timeout {
                url: url.to_string(),
                inactivity_secs: 10,
            }),
            seen_navigations: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn json_response(url: &str, body: &str) -> CapturedResponse {
        CapturedResponse {
            url: url.to_string(),
            mime_type: "application/json".to_string(),
            body: body.to_string(),
        }
    }

    fn seen(&self) -> Vec<(String, String)> {
        self.seen_navigations.lock().unwrap().clone()
    }
}

impl BrowserSession for FakeSession {
    fn navigate_collecting_json(
        &self,
        url: &str,
        url_filter: &str,
        _inactivity: Duration,
        _accept: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Vec<CapturedResponse>, BrowserError> {
        self.seen_navigations
            .lock()
            .unwrap()
            .push((url.to_string(), url_filter.to_string()));
        match &self.navigate_error {
            Some(error) => Err(error.clone()),
            None => Ok(self.responses.clone()),
        }
    }

    fn evaluate(&self, script: &str) -> Result<String, BrowserError> {
        if script == BOT_PROTECTION_PROBE_SCRIPT {
            if let Some(next) = self.probe_sequence.lock().unwrap().pop_front() {
                return Ok(next);
            }
            return Ok(self.probe_result.clone());
        }
        assert_eq!(script, MOBALYTICS_PRELOADED_STATE_SCRIPT);
        Ok(self.evaluate_result.clone())
    }

    fn window_is_shown(&self) -> bool {
        self.window_is_shown
    }
}

/// The inner `response_data` text of a D2Core planner response — the
/// exact shape `d4lc_rules::import_d2core_build_variant` parses.
fn d2core_planner_response_data(build_id: &str, title: &str) -> String {
    format!(
        r#"{{"data":{{"_id":"{build_id}","title":"{title}","variants":[{{"name":"Endgame","gear":{{}}}}]}}}}"#
    )
}

fn temp_builds_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("d4lc-build-download-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

// -----------------------------------------------------------------------
// D2Core gates + flow.
// -----------------------------------------------------------------------

#[test]
fn validate_d2core_build_id_enforces_the_upstream_four_character_gate() {
    assert!(validate_d2core_build_id("abcd").is_ok());
    assert!(validate_d2core_build_id("a1b2").is_ok());
    assert!(validate_d2core_build_id("").is_err());
    assert!(validate_d2core_build_id("abc").is_err());
    assert!(validate_d2core_build_id("abcde").is_err());
}

#[test]
fn build_d2core_planner_url_matches_upstream_navigation() {
    assert_eq!(
        build_d2core_planner_url("abcd"),
        "https://www.d2core.com/d4/planner?bd=abcd"
    );
}

#[test]
fn download_d2core_build_saves_response_data_verbatim() {
    let inner = d2core_planner_response_data("abcd", "Whirlwind Arsenal");
    // The CDP body wraps the inner string as a JSON string value under
    // `data`, per the upstream envelope `D2CoreBuildJson`.
    let body = format!(
        "{{\"data\":{{\"response_data\":{}}}}}",
        serde_json::to_string(&inner).unwrap()
    );
    let session = FakeSession::with_responses(vec![
        // The profile RPC response upstream ignores.
        FakeSession::json_response(
            "https://api.d2core.com/d4/web?env=diablocore&function-user-querycurrentuserinfo",
            r#"{"response_data":"{\"data\":{\"user\":\"someone\"}}"}"#,
        ),
        // The planner response upstream accepts.
        FakeSession::json_response(
            "https://api.d2core.com/d4/web?env=diablocore&function-planner-queryplan",
            &body,
        ),
    ]);
    let dir = temp_builds_dir("d2core-verbatim");

    let outcome = download_d2core_build("abcd", &dir, &session).expect("download");

    assert_eq!(
        session.seen(),
        vec![(
            "https://www.d2core.com/d4/planner?bd=abcd".to_string(),
            D2CORE_SNIFF_URL_FILTER.to_string()
        )],
        "must navigate the exact upstream planner URL with the upstream sniff filter"
    );
    assert_eq!(outcome.build_id, "abcd");
    assert_eq!(outcome.build_name.as_deref(), Some("Whirlwind Arsenal"));
    assert_eq!(
        outcome.path,
        dir.join("abcd.json"),
        "file name must be the build id like upstream .\\Builds\\D2Core\\{{id}}.json"
    );
    // Verbatim: the stored bytes equal the inner `response_data`
    // string, unmodified and without the CDP envelope.
    let stored = std::fs::read_to_string(&outcome.path).unwrap();
    assert_eq!(stored, inner);
    // The stored shape is what the d4lc-rules D2Core import parses.
    let parsed: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(parsed["data"]["_id"], "abcd");
    assert_eq!(parsed["data"]["title"], "Whirlwind Arsenal");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn download_d2core_build_ignores_foreign_envelopes_and_writes_nothing() {
    // A planner response for a DIFFERENT build must be rejected: the
    // envelope needle carries the requested id.
    let session = FakeSession::with_responses(vec![FakeSession::json_response(
        "https://api.d2core.com/d4/web?env=diablocore",
        &{
            let inner = d2core_planner_response_data("zzzz", "Other Build");
            format!(
                "{{\"response_data\":{}}}",
                serde_json::to_string(&inner).unwrap()
            )
        },
    )]);
    let dir = temp_builds_dir("d2core-foreign");

    let error = download_d2core_build("abcd", &dir, &session)
        .expect_err("a foreign envelope must be rejected");

    assert_eq!(error.kind, ImportErrorKind::Timeout);
    assert!(
        error
            .message
            .contains("D2Core sent no planner data for build 'abcd'"),
        "{error}"
    );
    assert!(!dir.join("abcd.json").exists(), "nothing may be written");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn download_d2core_build_surfaces_the_timeout_without_writing() {
    // The 10 s inactivity timer (upstream BuildsManagerD2Core.cs:52-53)
    // must surface as a structured error and write nothing.
    let session = FakeSession::timing_out("https://www.d2core.com/d4/planner?bd=abcd");
    let dir = temp_builds_dir("d2core-timeout");

    let error =
        download_d2core_build("abcd", &dir, &session).expect_err("a timed-out sniff must fail");

    assert_eq!(error.kind, ImportErrorKind::Timeout);
    assert!(error.message.contains("inactivity"), "{error}");
    assert!(
        error.message.contains("D2Core planner download for 'abcd'"),
        "{error}"
    );
    assert!(
        !dir.exists(),
        "no directory may be created for a rejected flow"
    );
}

#[test]
fn download_d2core_build_rejects_unsafe_ids_without_writing() {
    // Port-side cache-file safety gate: 4 characters, but not a safe
    // file name.
    let session = FakeSession::with_responses(Vec::new());
    let dir = temp_builds_dir("d2core-unsafe");

    let error = download_d2core_build("../x", &dir, &session)
        .expect_err("a path-bearing id must be rejected");

    assert!(
        error
            .message
            .contains("cannot be used as a cache file name"),
        "{error}"
    );
    assert!(!dir.exists());
}

// -----------------------------------------------------------------------
// D4Builds scrape + download.
// -----------------------------------------------------------------------

/// Test double standing in for the headless browser on the D4Builds
/// page: answers `evaluate` calls from a needle table (the first entry
/// whose needles ALL occur in the script wins) and records which
/// needles were hit, so tests can assert exactly which extraction
/// scripts ran.
struct ScriptedD4BuildsSession {
    answers: Vec<(&'static [&'static str], String)>,
    seen_navigations: std::sync::Mutex<Vec<String>>,
    hit_needles: std::sync::Mutex<Vec<&'static str>>,
}

impl ScriptedD4BuildsSession {
    fn new(answers: Vec<(&'static [&'static str], String)>) -> Self {
        Self {
            answers,
            seen_navigations: std::sync::Mutex::new(Vec::new()),
            hit_needles: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn hit(&self, needle: &str) -> bool {
        self.hit_needles.lock().unwrap().contains(&needle)
    }
}

impl BrowserSession for ScriptedD4BuildsSession {
    fn navigate_collecting_json(
        &self,
        url: &str,
        _url_filter: &str,
        _inactivity: Duration,
        _accept: Option<&dyn Fn(&str) -> bool>,
    ) -> Result<Vec<CapturedResponse>, BrowserError> {
        self.seen_navigations.lock().unwrap().push(url.to_string());
        Ok(Vec::new())
    }

    fn evaluate(&self, script: &str) -> Result<String, BrowserError> {
        let (needles, answer) = self
            .answers
            .iter()
            .find(|(needles, _)| needles.iter().all(|needle| script.contains(needle)))
            .ok_or_else(|| BrowserError::Session(format!("unexpected script: {script}")))?;
        self.hit_needles
            .lock()
            .unwrap()
            .extend(needles.iter().copied());
        Ok(answer.clone())
    }
}

/// Needle table for a well-formed two-variant page. Variant A carries
/// gear + paragon data, variant B only the gear tab content.
fn scripted_session() -> ScriptedD4BuildsSession {
    const GEAR_JSON: &str = r#"{
            "aspects": ["Bold Chieftain's Aspect"],
            "uniques": ["Harlequin Crest"],
            "runes": ["Yul"],
            "groups": {
                "Helm": [
                    {"text": "Implicit: +18% Movement Speed", "is_implicit": true},
                    {"text": "+950 Maximum Life", "is_greater": true}
                ],
                "Boots": [
                    {"text": "Attack Speed [12.5%] (tempered)", "is_tempered": true}
                ],
                "Ring1": [{"text": "+12% Attack Speed"}],
                "Ring2": [{"text": "+12% Attack Speed"}]
            }
        }"#;
    const PARAGON_JSON: &str = r#"[
            {
                "name": "Starting Board\n720",
                "glyph": "(Marshal)",
                "style": "transform: rotate(90deg);",
                "tiles": [
                    "paragon__board__tile r2 c10 active enabled",
                    "paragon__board__tile r1 c1",
                    "paragon__board__tile r5 c5 active"
                ]
            }
        ]"#;
    ScriptedD4BuildsSession::new(vec![
        (D4BUILDS_READY_NEEDLES, "[2,4,1,1]".to_string()),
        (D4BUILDS_NAME_NEEDLES, "Whirlwind Endgame".to_string()),
        (
            D4BUILDS_LAST_UPDATED_NEEDLES,
            "Last updated 3 days ago".to_string(),
        ),
        (D4BUILDS_GEAR_NEEDLES, GEAR_JSON.to_string()),
        (D4BUILDS_PARAGON_NEEDLES, PARAGON_JSON.to_string()),
        (D4BUILDS_TAB_NEEDLES, "3".to_string()),
        (D4BUILDS_OPTION_CLICK_NEEDLES, "ok".to_string()),
        (D4BUILDS_ARROW_NEEDLES, "ok".to_string()),
        (
            D4BUILDS_OPTIONS_NEEDLES,
            r#"["Endgame", "Speedfarm"]"#.to_string(),
        ),
    ])
}

// The needle groups mirror the decisive substrings of the extraction
// scripts; the first group whose needles ALL occur in a script wins,
// so more specific groups must come first.
const D4BUILDS_READY_NEEDLES: &[&str] = &["querySelectorAll('.builder__gem__slot').length"];
const D4BUILDS_NAME_NEEDLES: &[&str] = &["renameBuild"];
const D4BUILDS_LAST_UPDATED_NEEDLES: &[&str] = &["builder__last__updated"];
const D4BUILDS_GEAR_NEEDLES: &[&str] = &["builder__stat"];
const D4BUILDS_PARAGON_NEEDLES: &[&str] = &["paragon__board"];
const D4BUILDS_TAB_NEEDLES: &[&str] = &["builder__navigation__link"];
const D4BUILDS_ARROW_NEEDLES: &[&str] = &["item__arrow__icon--variant"];
const D4BUILDS_OPTIONS_NEEDLES: &[&str] = &["dropdown__option"];
const D4BUILDS_OPTION_CLICK_NEEDLES: &[&str] = &["dropdown__option", ".click("];

fn workspace_repo_root() -> PathBuf {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    root.pop();
    root.pop();
    root
}

fn temp_d4builds_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("d4lc-d4builds-download-{label}"));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn collect_statuses(
    statuses: &std::cell::RefCell<Vec<String>>,
    status: &str,
    _build_name: &str,
    _variants: Vec<String>,
) {
    statuses.borrow_mut().push(status.to_string());
}

#[test]
fn build_d4builds_page_url_matches_upstream_navigation() {
    assert_eq!(
        build_d4builds_page_url("whirlwind-endgame"),
        "https://d4builds.gg/builds/whirlwind-endgame/?var=0"
    );
}

#[test]
fn download_d4builds_build_scrapes_converts_stores_and_reports_upstream_statuses() {
    let session = scripted_session();
    let dir = temp_d4builds_dir("ok");
    let statuses = std::cell::RefCell::new(Vec::new());
    let sink = |status: &str, name: &str, variants: Vec<String>| {
        collect_statuses(&statuses, status, name, variants)
    };

    let outcome = download_d4builds_build(
        "ancients-barbarian-endgame",
        &dir,
        &session,
        &workspace_repo_root(),
        true,
        &sink,
    )
    .expect("download");

    assert_eq!(
        session.seen_navigations.lock().unwrap().as_slice(),
        ["https://d4builds.gg/builds/ancients-barbarian-endgame/?var=0"],
        "must navigate the exact upstream page URL"
    );
    // The upstream status sequence, byte for byte.
    assert_eq!(
        statuses.borrow().as_slice(),
        [
            "Downloading ancients-barbarian-endgame.",
            "Downloaded Whirlwind Endgame.",
            "Exporting Endgame.",
            "Paragon: Starting Board (Marshal).",
            "Exported Endgame.",
            "Exporting Speedfarm.",
            "Paragon: Starting Board (Marshal).",
            "Exported Speedfarm.",
            "Converting Endgame.",
            "Converted Endgame.",
            "Converting Speedfarm.",
            "Converted Speedfarm.",
        ],
        "statuses: {:?}",
        statuses.borrow()
    );
    assert_eq!(outcome.build_name, "Whirlwind Endgame");
    assert_eq!(
        outcome.variant_names,
        vec!["Endgame".to_string(), "Speedfarm".to_string()]
    );
    assert_eq!(outcome.path, dir.join("ancients-barbarian-endgame.json"));

    // The stored entity is the importable upstream shape.
    let stored = std::fs::read_to_string(&outcome.path).expect("stored entity");
    let entity: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(entity["Id"], "ancients-barbarian-endgame");
    assert_eq!(entity["Date"], "Last updated 3 days ago");
    assert_eq!(entity["Variants"].as_array().unwrap().len(), 2);
    let endgame = &entity["Variants"][0];
    // Implicit text is cleaned after the ':' and stripped of digits.
    assert_eq!(endgame["Helm"][0]["AffixText"], "Movement Speed");
    assert_eq!(endgame["Helm"][0]["IsImplicit"], true);
    // Tempered text drops the bracket/paren groups.
    assert_eq!(endgame["Boots"][0]["AffixText"], "Attack Speed");
    assert_eq!(endgame["Boots"][0]["IsTempered"], true);
    // Both rings keep their entries like upstream's AddRange order.
    assert_eq!(endgame["Ring"].as_array().unwrap().len(), 2);
    // Paragon: rotate(90deg) transforms r2/c10 → x19/y9 and r5/c5 →
    // x16/y4 on the 21x21 grid.
    let board = &endgame["ParagonBoards"][0];
    assert_eq!(board["Name"], "Starting Board");
    assert_eq!(board["Glyph"], "Marshal");
    assert_eq!(board["Rotation"], "90°");
    let nodes: Vec<bool> = serde_json::from_value(board["Nodes"].clone()).unwrap();
    assert_eq!(nodes.len(), 21 * 21);
    assert!(nodes[9 * 21 + 19], "r2/c10 active lands at index 208");
    assert!(nodes[4 * 21 + 16], "r5/c5 active lands at index 100");
    assert_eq!(nodes.iter().filter(|active| **active).count(), 2);

    // The dialog refresh lists the downloaded build (left list).
    let builds = crate::actions::build_import_dialogs::load_builds_for_provider(
        crate::actions::build_import_dialogs::PROVIDER_D4BUILDS,
        &dir,
    )
    .expect("enumerate downloaded build");
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].id, "ancients-barbarian-endgame");
    assert_eq!(
        builds[0].variant_names,
        vec!["Endgame".to_string(), "Speedfarm".to_string()]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn download_d4builds_build_without_paragon_never_touches_the_paragon_tab() {
    let session = scripted_session();
    let dir = temp_d4builds_dir("no-paragon");
    let statuses = std::cell::RefCell::new(Vec::new());
    let sink = |status: &str, name: &str, variants: Vec<String>| {
        collect_statuses(&statuses, status, name, variants)
    };

    let outcome = download_d4builds_build(
        "ancients-barbarian-endgame",
        &dir,
        &session,
        &workspace_repo_root(),
        false,
        &sink,
    )
    .expect("download");

    assert!(
        !session.hit("paragon__board"),
        "the paragon extraction must not run when the import toggle is off"
    );
    assert!(
        !statuses
            .borrow()
            .iter()
            .any(|status| status.starts_with("Paragon:")),
        "no Paragon status may be emitted"
    );
    let stored = std::fs::read_to_string(&outcome.path).unwrap();
    let entity: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert!(
        entity["Variants"][0]["ParagonBoards"]
            .as_array()
            .unwrap()
            .is_empty(),
        "the stored variant carries no paragon boards"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn download_d4builds_build_reports_the_missing_name_marker() {
    // A page that never exposes the build name must fail like
    // upstream's WebDriverWait timeout, writing nothing.
    let session = ScriptedD4BuildsSession::new(vec![(
        D4BUILDS_GEAR_NEEDLES,
        r#"{"aspects":[],"uniques":[],"runes":[],"groups":{}}"#.to_string(),
    )]);
    let dir = temp_d4builds_dir("no-name");

    // The name poll itself never resolves because no answer matches the
    // name script: the session rejects it.
    let error = download_d4builds_build(
        "ancients-barbarian-endgame",
        &dir,
        &session,
        &workspace_repo_root(),
        false,
        &|_, _, _| {},
    )
    .expect_err("a page without the marker must fail");

    assert!(error.message.contains("D4Builds page script"), "{error}");
    assert!(!dir.join("ancients-barbarian-endgame.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn download_d4builds_build_rejects_unsafe_ids_before_any_browser_work() {
    let session = scripted_session();
    let dir = temp_d4builds_dir("unsafe-id");
    let error = download_d4builds_build(
        "../escape",
        &dir,
        &session,
        &workspace_repo_root(),
        false,
        &|_, _, _| {},
    )
    .expect_err("a path-bearing id must be rejected");
    assert!(
        error
            .message
            .contains("cannot be used as a cache file name"),
        "{error}"
    );
    assert!(
        session.seen_navigations.lock().unwrap().is_empty(),
        "the safety gate must fire before navigation"
    );
}

// -----------------------------------------------------------------------
// Mobalytics download flows (build id / slug / profile).
// -----------------------------------------------------------------------

/// A minimal `userGeneratedDocumentById` GraphQL body for the CDP id
/// flow: one variant ("Mobalytics Live Build") with one greater gear
/// stat and one implicit on the gloves slot. The document pins its own
/// id (the stored file is named by it).
fn mobalytics_graphql_body() -> String {
    r#"{"data":{"game":{"documents":{"userGeneratedDocumentById":{"data":{"id":"a1b2c3d4-1111-2222-3333-444455556666","updatedAt":"2026-09-01T10:00:00Z","data":{"name":"Mobalytics Live Build","buildVariants":{"values":[{"id":"v1","genericBuilder":{"slots":[{"gameSlotSlug":"gloves","gameEntity":{"slug":"generic-gloves","type":"legendaryItems","modifiers":{"gearStats":[{"id":"attack-speed","isGreater":true}],"implicitStats":[{"id":"movement-speed"}],"socketStats":[],"temperingStats":[]}}}]}}]}},"content":[]}}}}}}"#.to_string()
}

/// The `window.__PRELOADED_STATE__` wrapper of the slug flow: the
/// by-slug document of the same shape.
fn mobalytics_slug_state() -> String {
    r#"{"diablo4State":{"apollo":{"graphqlV2":{"queries":[{"state":{"data":[{"game":{"documents":{"userGeneratedDocumentBySlug":{"data":{"id":"slug-doc-1","updatedAt":"2026-09-02T11:00:00Z","data":{"name":"Slug Flow Build","buildVariants":{"values":[{"id":"v1"}]}},"content":[]}}}}}]}}]}}}}"#.to_string()
}

/// The `window.__PRELOADED_STATE__` of the profile flow: two published
/// builds (the first one newer) plus the mgp-header user.
const MOBALYTICS_PROFILE_STATE: &str = r#"{"diablo4State":{"apollo":{"graphqlV2":{"queries":[{"queryKey":["ngf-creator-profile-documents"],"state":{"data":{"pages":[[{"game":{"documents":{"userGeneratedDocuments":{"documents":[{"id":"moba-live-001","slugifiedName":"spiritborn-endgame","updatedAt":"2026-05-31T12:00:00Z","data":{"name":"Spiritborn Endgame"}},{"id":"moba-live-002","slugifiedName":"","updatedAt":"2026-05-30T12:00:00Z","data":{"name":"Warlock Starter"}}]}}}}]]}}},{"queryKey":["mgp-header"],"state":{"data":[{"mgp":{"profile":{"data":{"user":{"displayName":"Test Profile","id":"profile-123","username":"test-profile"}}}}}]}}]}}}}"#;

#[test]
fn download_mobalytics_build_stores_the_converted_entity_from_the_graphql_response() {
    let build_id = "a1b2c3d4-1111-2222-3333-444455556666";
    let session = FakeSession::with_responses(vec![
        // A JSON body without the upstream prefix is ignored.
        FakeSession::json_response(
            "https://api.mobalytics.gg/api/diablo4/other",
            r#"{"data":{"game":{"other":1}}}"#,
        ),
        FakeSession::json_response(
            "https://api.mobalytics.gg/api/diablo4/graphql",
            &mobalytics_graphql_body(),
        ),
    ]);
    let dir = temp_builds_dir("mobalytics-id-flow");
    let profiles_dir = temp_builds_dir("mobalytics-id-flow-profiles");
    let statuses = std::cell::RefCell::new(Vec::new());
    let sink = |status: &str, name: &str, variants: Vec<String>| {
        collect_statuses(&statuses, status, name, variants)
    };

    let outcome = download_mobalytics_build(
        // Mixed case proves the upstream `buildUrl.ToLower()` parity.
        // The id flow serves `/builds/{GUID}` pages — profile + builds
        // with a GUID segment, which is NOT slug format upstream.
        "https://mobalytics.gg/diablo-4/profile/Some-User/builds/a1B2c3d4-1111-2222-3333-444455556666",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &sink,
    )
    .expect("download");

    assert_eq!(
        session.seen(),
        vec![(
            "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666".to_string(),
            MOBALYTICS_SNIFF_URL_FILTER.to_string()
        )],
        "must navigate the LOWERCASED url with the upstream api/diablo4 filter"
    );
    assert_eq!(
        statuses.borrow().as_slice(),
        [
            "Downloading https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666.",
            "Exporting Mobalytics Live Build.",
            "Exporting Mobalytics Live Build.",
            "Exported Mobalytics Live Build.",
            "Converting Mobalytics Live Build.",
            "Converted Mobalytics Live Build.",
        ],
        "statuses: {:?}",
        statuses.borrow()
    );
    let path = outcome.path;
    assert_eq!(path, dir.join(format!("{build_id}.json")));
    assert_eq!(outcome.name, "Mobalytics Live Build");
    assert!(outcome.log_lines.is_empty(), "{:?}", outcome.log_lines);

    // The stored entity is the importable converted shape.
    let stored = std::fs::read_to_string(&path).expect("stored entity");
    let entity: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(entity["Id"], build_id);
    assert_eq!(entity["Name"], "Mobalytics Live Build");
    assert_eq!(
        entity["Url"],
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666"
    );
    assert_eq!(entity["Date"], "2026-09-01T10:00:00Z");
    let variant = &entity["Variants"][0];
    assert_eq!(variant["Helm"].as_array().unwrap().len(), 0);
    let gloves = &variant["Gloves"];
    assert_eq!(gloves[0]["AffixText"], "attack-speed");
    assert_eq!(gloves[0]["AffixTextList"].as_array().unwrap().len(), 2);
    assert_eq!(gloves[0]["IsGreater"], true);
    assert_eq!(gloves[1]["AffixText"], "movement-speed");
    assert_eq!(gloves[1]["IsImplicit"], true);

    // The dialog refresh lists the downloaded build (left list).
    let builds = crate::actions::build_import_dialogs::load_builds_for_provider(
        crate::actions::build_import_dialogs::PROVIDER_MOBALYTICS,
        &dir,
    )
    .expect("enumerate downloaded build");
    assert_eq!(builds.len(), 1);
    assert_eq!(builds[0].id, build_id);
    assert_eq!(
        builds[0].url,
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666",
        "the Update flow re-downloads by the stored Url"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn download_mobalytics_build_reports_paragon_boards_without_nodes() {
    // BuildsManagerMobalytics.cs:967-974: an ErrorOccurredMessage per
    // exported board that has no node slug.
    let body = mobalytics_graphql_body().replace(
        r#""genericBuilder""#,
        r#""paragon":{"boards":[{"board":{"slug":"barbarian-starter-board"},"rotation":0}],"nodes":[]},"genericBuilder""#,
    );
    let session = FakeSession::with_responses(vec![FakeSession::json_response(
        "https://api.mobalytics.gg/api/diablo4/graphql",
        &body,
    )]);
    let dir = temp_builds_dir("mobalytics-no-nodes");
    let profiles_dir = temp_builds_dir("mobalytics-no-nodes-profiles");
    let url = "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666";

    let outcome = download_mobalytics_build(
        url,
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &|_, _, _| {},
    )
    .expect("download");
    assert_eq!(
        outcome.log_lines,
        ["No nodes found for paragon board barbarian-starting-board."]
    );

    let session = FakeSession::with_responses(vec![FakeSession::json_response(
        "https://api.mobalytics.gg/api/diablo4/graphql",
        &body,
    )]);
    let gated = download_mobalytics_build(
        url,
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        false,
        &|_, _, _| {},
    )
    .expect("download");
    assert!(gated.log_lines.is_empty(), "no paragon export, no message");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn download_mobalytics_slug_flow_parses_the_preloaded_state_document() {
    let session =
        ScriptedD4BuildsSession::new(vec![(&["__PRELOADED_STATE__"], mobalytics_slug_state())]);
    let dir = temp_builds_dir("mobalytics-slug-flow");
    let profiles_dir = temp_builds_dir("mobalytics-slug-flow-profiles");
    let statuses = std::cell::RefCell::new(Vec::new());
    let sink = |status: &str, name: &str, variants: Vec<String>| {
        collect_statuses(&statuses, status, name, variants)
    };

    let outcome = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/builds/warlock-dread-claws",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &sink,
    )
    .expect("slug download");

    assert_eq!(
        session.seen_navigations.lock().unwrap().as_slice(),
        ["https://mobalytics.gg/diablo-4/builds/warlock-dread-claws"],
        "the slug flow navigates the user-supplied url"
    );
    assert_eq!(
        statuses.borrow().as_slice(),
        [
            "Downloading https://mobalytics.gg/diablo-4/builds/warlock-dread-claws.",
            "Exporting Slug Flow Build.",
            "Exporting Slug Flow Build.",
            "Exported Slug Flow Build.",
            "Converting Slug Flow Build.",
            "Converted Slug Flow Build.",
        ],
        "statuses: {:?}",
        statuses.borrow()
    );
    let path = outcome.path;
    assert_eq!(
        path,
        dir.join("slug-doc-1.json"),
        "file named by the doc id"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn download_mobalytics_profile_flow_stores_the_display_name_file() {
    let session = FakeSession {
        evaluate_result: MOBALYTICS_PROFILE_STATE.to_string(),
        ..FakeSession::with_responses(Vec::new())
    };
    let dir = temp_builds_dir("mobalytics-profile-flow");
    let profiles_dir = temp_builds_dir("mobalytics-profile-flow-profiles");
    let statuses = std::cell::RefCell::new(Vec::new());
    let sink = |status: &str, name: &str, variants: Vec<String>| {
        collect_statuses(&statuses, status, name, variants)
    };

    let outcome = download_mobalytics_build(
        // The query filter must surface in the profile display name
        // and the stored file name.
        "https://mobalytics.gg/diablo-4/profile/test-profile?class=campaign",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &sink,
    )
    .expect("profile download");

    assert_eq!(
        statuses.borrow().as_slice(),
        [
            "Downloading https://mobalytics.gg/diablo-4/profile/test-profile?class=campaign.",
            "Exporting Test Profile - campaign.",
            "Exported Spiritborn Endgame.",
            "Exported Warlock Starter.",
        ],
        "statuses: {:?}",
        statuses.borrow()
    );
    let path = outcome.path;
    assert_eq!(
        path,
        profiles_dir.join("Test Profile - campaign.json"),
        "the profile file carries the display name incl. the filter suffix"
    );
    let stored = std::fs::read_to_string(&path).unwrap();
    let profile: serde_json::Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(profile["Id"], "profile-123");
    assert_eq!(profile["Name"], "Test Profile - campaign");
    assert_eq!(profile["ProfileName"], "test-profile");
    let variants = profile["Variants"].as_array().unwrap();
    assert_eq!(variants.len(), 2);
    // Date descending sort.
    assert_eq!(variants[0]["Name"], "Spiritborn Endgame");
    assert_eq!(
        variants[0]["Url"],
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds/spiritborn-endgame"
    );
    assert_eq!(variants[1]["Name"], "Warlock Starter");
    assert_eq!(
        variants[1]["Url"],
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds/moba-live-002",
        "an empty slugifiedName falls back to the build id"
    );

    // The dialog Profiles sub-tab lists the stored profile.
    let profiles = crate::actions::build_import_dialogs::load_profiles_for_provider(
        crate::actions::build_import_dialogs::PROVIDER_MOBALYTICS,
        &profiles_dir,
    )
    .expect("enumerate stored profile")
    .0;
    assert_eq!(profiles.len(), 1);
    assert_eq!(profiles[0].name, "Test Profile - campaign");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn download_mobalytics_profile_flow_without_profile_queries_fails_without_storing() {
    // The state exists but carries neither required query (the live
    // `/profile/<user>` shell): upstream sends "Failed. Invalid json." and
    // completes; the port reports the invalid response as a failure.
    let session = FakeSession {
        evaluate_result: r#"{"diablo4State":{"apollo":{"graphqlV2":{"queries":[]}}}}"#.to_string(),
        ..FakeSession::with_responses(Vec::new())
    };
    let dir = temp_builds_dir("mobalytics-invalid-json");
    let profiles_dir = temp_builds_dir("mobalytics-invalid-json-profiles");
    let statuses = std::cell::RefCell::new(Vec::new());
    let sink = |status: &str, name: &str, variants: Vec<String>| {
        collect_statuses(&statuses, status, name, variants)
    };

    let error = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/profile/test-profile",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &sink,
    )
    .expect_err("a profile page without profile data is no import");

    assert_eq!(error.kind, ImportErrorKind::InvalidResponse);
    assert!(error.message.contains("carried no profile data"), "{error}");
    assert!(!profiles_dir.exists(), "nothing may be stored");
    assert_eq!(
        session.seen()[0].0,
        "https://mobalytics.gg/diablo-4/profile/test-profile/builds",
        "a bare profile URL loads its builds page (the site's own redirect)"
    );
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn download_mobalytics_id_flow_surfaces_the_timeout_as_a_failure() {
    // The session timeout IS upstream's inactivity timer. Upstream only
    // sends "Timeout occurred." and completes; the port reports a timeout
    // failure (no build was stored).
    let session = FakeSession::timing_out(
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666",
    );
    let dir = temp_builds_dir("mobalytics-id-timeout");
    let profiles_dir = temp_builds_dir("mobalytics-id-timeout-profiles");

    let error = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &|_, _, _| {},
    )
    .expect_err("a timeout is a failure");

    assert_eq!(error.kind, ImportErrorKind::Timeout);
    assert!(!dir.exists(), "no directory may be created");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn download_mobalytics_id_flow_without_a_document_response_times_out() {
    // JSON responses arrived, but none carries the upstream prefix:
    // the inactivity timer fires like upstream (:264 acceptance).
    let session = FakeSession::with_responses(vec![FakeSession::json_response(
        "https://api.mobalytics.gg/api/diablo4/other",
        r#"{"data":{"game":{"other":1}}}"#,
    )]);
    let dir = temp_builds_dir("mobalytics-no-doc");
    let profiles_dir = temp_builds_dir("mobalytics-no-doc-profiles");

    let error = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        true,
        &|_, _, _| {},
    )
    .expect_err("a missing document is a failure");

    assert_eq!(error.kind, ImportErrorKind::Timeout);
    assert!(
        error.message.contains("userGeneratedDocumentById"),
        "{error}"
    );
    assert!(!dir.exists());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn mobalytics_url_classification_matches_the_upstream_checks() {
    // Profile page: /profile/ without /builds/.
    assert!(mobalytics_is_profile_url(
        "https://mobalytics.gg/diablo-4/profile/some-user"
    ));
    assert!(!mobalytics_is_profile_url(
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/my-build"
    ));
    // HasBuildUrlSlugFormat: builds without profile; both words only
    // when no 36-char/4-hyphen GUID segment exists.
    assert!(has_build_url_slug_format(
        "https://mobalytics.gg/diablo-4/builds/warlock-dread-claws"
    ));
    assert!(has_build_url_slug_format(
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/tyrants-grasp-spam-warlock"
    ));
    assert!(!has_build_url_slug_format(
        "https://mobalytics.gg/diablo-4/profile/some-user/builds/a1b2c3d4-1111-2222-3333-444455556666"
    ));
    assert!(!has_build_url_slug_format(
        "https://mobalytics.gg/diablo-4/profile/some-user"
    ));
}

#[test]
fn browser_profiles_keep_chromiums_sandbox_enabled() {
    for profile in [
        BrowserLaunchProfile::D2Core,
        BrowserLaunchProfile::D4Builds,
        mobalytics_launch_profile(false),
        mobalytics_launch_profile(true),
    ] {
        assert!(BASE_CHROME_ARGS
            .iter()
            .chain(profile.extra_args())
            .all(|arg| !matches!(*arg, "--no-sandbox" | "--disable-setuid-sandbox")));
    }
}

#[test]
fn mobalytics_launch_profile_skips_headless_for_the_show_browser_toggle() {
    // Upstream InitSelenium adds --headless only while
    // IsShowBrowserMobalyticsEnabled is false.
    assert_eq!(
        mobalytics_launch_profile(false),
        BrowserLaunchProfile::Mobalytics { headless: true }
    );
    assert_eq!(
        mobalytics_launch_profile(true),
        BrowserLaunchProfile::Mobalytics { headless: false }
    );
}

// -----------------------------------------------------------------------
// Feature-off capability error.
// -----------------------------------------------------------------------

#[test]
#[cfg(not(feature = "browser-download"))]
fn production_session_reports_the_missing_feature() {
    // Fail-closed invariant: without the feature the production
    // constructor must report the capability gap, never fake a
    // browser.
    let error = match launch_chrome_session(BrowserLaunchProfile::D2Core, &ImportControl::new()) {
        Err(error) => error,
        Ok(_) => panic!("no browser may be launched without the feature"),
    };
    assert!(matches!(error, BrowserError::FeatureDisabled));
    assert!(error.to_string().contains("--features browser-download"));
}

// -----------------------------------------------------------------------
// Opt-in live smoke (feature build only).
// -----------------------------------------------------------------------

#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only"]
fn browser_captures_the_initial_json_response() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Instant;

    let session = launch_chrome_session(BrowserLaunchProfile::D2Core, &ImportControl::new())
        .expect("launch browser");
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let url = format!(
        "http://{}/build.json",
        listener.local_addr().expect("address")
    );
    let server = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .expect("timeout");
                    let mut request = [0; 4096];
                    let bytes_read = stream.read(&mut request).unwrap_or(0);
                    assert!(bytes_read > 0, "browser closed before sending a request");
                    let body = r#"{"build":"local-regression"}"#;
                    write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).expect("send JSON");
                    return;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        Instant::now() < deadline,
                        "browser did not request the page"
                    );
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("accept: {error}"),
            }
        }
    });
    let responses =
        session.navigate_collecting_json(&url, "/build.json", Duration::from_millis(500), None);
    server.join().expect("local server");
    let responses = responses.expect("collect response");
    assert!(responses
        .iter()
        .any(|response| response.url == url && response.body == r#"{"build":"local-regression"}"#));
}

#[test]
#[cfg(feature = "browser-download")]
#[ignore = "manual smoke: requires network, a local Chromium, and D4LC_D2CORE_SMOKE_ID"]
fn d2core_live_download_smoke() {
    // Opt-in end-to-end check against the live planner through a real
    // headless Chromium:
    //   cargo test -p d4lc-app --features browser-download \
    //       d2core_live_download_smoke -- --ignored
    // The planner id is NOT invented: take it from an actual planner
    // URL opened on d2core.com (`…/d4/planner?bd=<id>`):
    //   D4LC_D2CORE_SMOKE_ID=abcd cargo test ...
    let raw_id = std::env::var("D4LC_D2CORE_SMOKE_ID").unwrap_or_default();
    assert!(
        !raw_id.trim().is_empty(),
        "set D4LC_D2CORE_SMOKE_ID=<4-character planner id> to run this smoke"
    );
    let session = launch_chrome_session(BrowserLaunchProfile::D2Core, &ImportControl::new())
        .expect("launch chromium");
    let dir = std::env::temp_dir().join("d4lc-d2core-live-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    let outcome =
        download_d2core_build(raw_id.trim(), &dir, session.as_ref()).expect("live download");
    assert!(outcome.path.is_file());
    // The dialog refresh reads the stored file back through the same
    // build loader the import path uses — the downloaded planner
    // build must yield an importable build row (download → store →
    // import).
    let builds = crate::actions::build_import_dialogs::load_builds_for_provider(
        crate::actions::build_import_dialogs::PROVIDER_D2CORE,
        &dir,
    )
    .expect("enumerate downloaded build");
    assert!(
        !builds.is_empty(),
        "the downloaded planner build must yield an importable build row"
    );
    eprintln!(
        "[smoke] d2core build '{}' stored at {} ({} variant row(s))",
        outcome.build_name.as_deref().unwrap_or(&outcome.build_id),
        outcome.path.display(),
        builds[0].variant_names.len()
    );
}

#[test]
#[cfg(feature = "browser-download")]
#[ignore = "manual smoke: requires network, a local Chromium, and D4LC_D4BUILDS_SMOKE_ID"]
fn d4builds_live_download_smoke() {
    // Opt-in end-to-end check against the live d4builds.gg build page
    // through a real headless Chromium:
    //   cargo test -p d4lc-app --features browser-download \
    //       d4builds_live_download_smoke -- --ignored
    // The build id is NOT invented: take it from an actual build page
    // opened on d4builds.gg (`https://d4builds.gg/builds/<id>`):
    //   D4LC_D4BUILDS_SMOKE_ID=<build id> cargo test ...
    // Setting D4LC_D4BUILDS_SMOKE_DUMP=<file> additionally writes the
    // raw DOM extraction (the d4lc-rules conversion fixture shape) so
    // a changed site can be re-recorded.
    let raw_id = std::env::var("D4LC_D4BUILDS_SMOKE_ID").unwrap_or_default();
    assert!(
        !raw_id.trim().is_empty(),
        "set D4LC_D4BUILDS_SMOKE_ID=<build id> to run this smoke"
    );
    let build_id = raw_id.trim();
    let session = launch_chrome_session(BrowserLaunchProfile::D4Builds, &ImportControl::new())
        .expect("launch chromium");
    let dir = std::env::temp_dir().join("d4lc-d4builds-live-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    let statuses: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let sink = |status: &str, _name: &str, _variants: Vec<String>| {
        statuses.lock().unwrap().push(status.to_string());
    };

    let site =
        scrape_d4builds_site_build(build_id, session.as_ref(), true, &sink).expect("live scrape");
    if let Ok(dump) = std::env::var("D4LC_D4BUILDS_SMOKE_DUMP") {
        if !dump.trim().is_empty() {
            let text = serde_json::to_string_pretty(&site).expect("serialize extraction");
            std::fs::write(dump.trim(), text).expect("write extraction dump");
            eprintln!("[smoke] site extraction dumped");
        }
    }
    let outcome = convert_and_store_d4builds_site(&site, &dir, &workspace_repo_root(), &sink)
        .expect("live conversion + store");

    // The dialog refresh reads the stored file back through the same
    // build loader the import path uses — the downloaded build must
    // yield an importable build row.
    let builds = crate::actions::build_import_dialogs::load_builds_for_provider(
        crate::actions::build_import_dialogs::PROVIDER_D4BUILDS,
        &dir,
    )
    .expect("enumerate downloaded build");
    assert!(
        !builds.is_empty(),
        "the downloaded D4Builds build must yield an importable build row"
    );
    eprintln!(
        "[smoke] d4builds build '{}' stored at {} ({} variant row(s), {} status lines)",
        outcome.build_name,
        outcome.path.display(),
        builds[0].variant_names.len(),
        statuses.lock().unwrap().len()
    );
}

#[test]
#[cfg(feature = "browser-download")]
#[ignore = "manual smoke: requires network, a local Chromium, and D4LC_MOBALYTICS_SMOKE_URL"]
fn mobalytics_live_download_smoke() {
    // Opt-in end-to-end check against the live mobalytics.gg site
    // through a real Chromium:
    //   cargo test -p d4lc-app --features browser-download \
    //       mobalytics_live_download_smoke -- --ignored
    // The URL is NOT invented: take it from an actual build or profile
    // page opened on mobalytics.gg, e.g.
    //   D4LC_MOBALYTICS_SMOKE_URL=https://mobalytics.gg/diablo-4/builds/<slug> \
    //       cargo test ...
    // Setting D4LC_MOBALYTICS_SMOKE_DUMP=<dir> additionally writes the
    // captured raw inputs (the sniffed api/diablo4 bodies and the
    // preloaded state) so the offline fixtures can be re-recorded.
    //
    // mobalytics.gg currently answers with a Cloudflare challenge (plain
    // HTTP 403, headless Chromium stuck on "Just a moment..."): the flow
    // then fails with the bot-protection error and stores nothing. d4lc
    // never tries to pass the challenge.
    let raw_url = std::env::var("D4LC_MOBALYTICS_SMOKE_URL").unwrap_or_default();
    assert!(
        !raw_url.trim().is_empty(),
        "set D4LC_MOBALYTICS_SMOKE_URL=<mobalytics build or profile url> to run this smoke"
    );
    let url = raw_url.trim();
    let session = launch_chrome_session(mobalytics_launch_profile(false), &ImportControl::new())
        .expect("launch chromium");
    let dir = std::env::temp_dir().join("d4lc-mobalytics-live-smoke");
    let _ = std::fs::remove_dir_all(&dir);
    let builds_dir = dir.join("Builds").join("Mobalytics");
    let profiles_dir = dir.join("Profiles").join("Mobalytics");
    let statuses: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let sink = |status: &str, _name: &str, _variants: Vec<String>| {
        statuses.lock().unwrap().push(status.to_string());
    };

    if let Ok(dump) = std::env::var("D4LC_MOBALYTICS_SMOKE_DUMP") {
        if !dump.trim().is_empty() {
            let dump_dir = std::path::PathBuf::from(dump.trim());
            std::fs::create_dir_all(&dump_dir).expect("create dump dir");
            let bodies = session
                .navigate_collecting_json(
                    &mobalytics_page_url(&url.to_lowercase()),
                    MOBALYTICS_SNIFF_URL_FILTER,
                    MOBALYTICS_CAPTURE_TIMEOUT,
                    Some(&|body: &str| body.starts_with(MOBALYTICS_GRAPHQL_PREFIX)),
                )
                .expect("live capture");
            for (index, captured) in bodies.iter().enumerate() {
                if captured.body.starts_with(MOBALYTICS_GRAPHQL_PREFIX) {
                    std::fs::write(
                        dump_dir.join(format!("graphql-{index}.json")),
                        &captured.body,
                    )
                    .expect("write graphql dump");
                }
            }
            if let Ok(state) = session.evaluate(MOBALYTICS_PRELOADED_STATE_SCRIPT) {
                if !state.is_empty() {
                    std::fs::write(dump_dir.join("preloaded-state.json"), state)
                        .expect("write preloaded state dump");
                }
            }
            eprintln!(
                "[smoke] mobalytics raw inputs dumped to {}",
                dump_dir.display()
            );
        }
    }

    let outcome = download_mobalytics_build(
        url,
        &builds_dir,
        &profiles_dir,
        session.as_ref(),
        &workspace_repo_root(),
        true,
        &sink,
    )
    .expect("live download");
    assert!(outcome.path.is_file(), "the stored JSON must exist");
    // A stored build must read back through the same loader the dialog
    // list uses (profiles are listed by their own loader).
    if outcome.path.starts_with(&builds_dir) {
        let builds = crate::actions::build_import_dialogs::load_builds_for_provider(
            crate::actions::build_import_dialogs::PROVIDER_MOBALYTICS,
            &builds_dir,
        )
        .expect("enumerate downloaded build");
        assert_eq!(builds.len(), 1, "the downloaded build must list");
        assert!(
            !builds[0].variant_names.is_empty(),
            "the build must carry variants"
        );
    }
    eprintln!(
        "[smoke] mobalytics '{}' stored at {} ({} variant row(s): {:?}, {} status lines)",
        outcome.name,
        outcome.path.display(),
        outcome.variant_names.len(),
        outcome.variant_names,
        statuses.lock().unwrap().len()
    );
}

// -----------------------------------------------------------------------
// Bot-protection challenge detection.
// -----------------------------------------------------------------------

/// The DOM headless Chromium rendered for a Mobalytics build listing
/// behind Cloudflare's managed challenge (provenance and redactions:
/// `tests/fixtures/bot-protection/README.md`).
const RECORDED_CLOUDFLARE_CHALLENGE: &str =
    include_str!("../../../tests/fixtures/bot-protection/mobalytics-cloudflare-challenge.dom.html");

fn recorded_challenge_probe() -> String {
    serde_json::json!({ "title": "Just a moment...", "html": RECORDED_CLOUDFLARE_CHALLENGE })
        .to_string()
}

#[test]
fn session_chrome_args_place_the_shown_mobalytics_window_on_screen() {
    let headless = session_chrome_args(mobalytics_launch_profile(false));
    assert!(headless.contains(&"--window-position=-32000,-32000"));
    let shown = session_chrome_args(mobalytics_launch_profile(true));
    assert!(!shown.contains(&"--window-position=-32000,-32000"));
    assert!(shown.contains(&"--window-size=1600,900"));
    assert!(shown.contains(&"--user-agent=Diablo4Companion/1.0"));
    // The other providers never show a window and keep the off-screen position.
    assert!(session_chrome_args(BrowserLaunchProfile::D2Core)
        .contains(&"--window-position=-32000,-32000"));
}

#[test]
fn the_challenge_message_ends_with_the_given_remedy() {
    let challenge = bot_protection_message(
        "Mobalytics",
        "https://mobalytics.gg/diablo-4/builds/druid-pulverize",
        BotProtectionPage::Challenge,
        "REMEDY.",
    );
    assert!(challenge.ends_with("REMEDY."), "{challenge}");
    // A block page has no remedy: retrying does not help.
    let blocked = bot_protection_message(
        "Mobalytics",
        "https://mobalytics.gg/diablo-4/builds/druid-pulverize",
        BotProtectionPage::Blocked,
        "REMEDY.",
    );
    assert!(!blocked.contains("REMEDY."), "{blocked}");
}

#[test]
fn a_headless_mobalytics_challenge_is_retried_once() {
    let session = FakeSession {
        probe_result: recorded_challenge_probe(),
        ..FakeSession::with_responses(Vec::new())
    };
    let dir = temp_builds_dir("mobalytics-retry");
    let profiles_dir = temp_builds_dir("mobalytics-retry-profiles");

    let error = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/builds/druid-pulverize",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        false,
        &|_, _, _| {},
    )
    .expect_err("a challenge page must fail the download");

    assert!(
        error.message.contains("bot-protection challenge"),
        "{error}"
    );
    assert_eq!(session.seen().len(), 2, "exactly one retry, never a loop");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn a_silent_challenge_page_is_named_instead_of_the_upstream_timeout() {
    let session = FakeSession {
        probe_result: recorded_challenge_probe(),
        ..FakeSession::timing_out("https://mobalytics.gg/diablo-4/builds/druid-pulverize")
    };
    let dir = temp_builds_dir("mobalytics-silent-challenge");
    let profiles_dir = temp_builds_dir("mobalytics-silent-challenge-profiles");

    let error = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/builds/druid-pulverize",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        false,
        &|_, _, _| {},
    )
    .expect_err("the challenge must be named");

    assert!(
        error.message.contains("bot-protection challenge"),
        "{error}"
    );
    assert!(!error.message.contains("Timeout occurred"), "{error}");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn the_shown_browser_is_waited_for_until_the_challenge_is_gone() {
    let session = FakeSession {
        window_is_shown: true,
        probe_sequence: std::sync::Mutex::new(
            [recorded_challenge_probe(), String::new()]
                .into_iter()
                .collect(),
        ),
        ..FakeSession::with_responses(Vec::new())
    };
    assert!(wait_for_bot_protection_to_clear(
        &session,
        Duration::from_secs(5),
        Duration::from_millis(1),
    )
    .unwrap());
    // A block page ends the wait at once: the user cannot clear it.
    let blocked = FakeSession {
        probe_result:
            serde_json::json!({ "title": "Attention Required! | Cloudflare", "html": "" })
                .to_string(),
        ..FakeSession::with_responses(Vec::new())
    };
    assert!(!wait_for_bot_protection_to_clear(
        &blocked,
        Duration::from_secs(5),
        Duration::from_millis(1),
    )
    .unwrap());
}

#[test]
fn bot_protection_detection_matches_the_recorded_cloudflare_challenge() {
    use BotProtectionPage::{Blocked, Challenge};

    assert_eq!(
        bot_protection_page("Just a moment...", RECORDED_CLOUDFLARE_CHALLENGE),
        Some(Challenge)
    );
    // The recorded markup without its title still matches.
    assert_eq!(
        bot_protection_page("", RECORDED_CLOUDFLARE_CHALLENGE),
        Some(Challenge)
    );
    // Every marker on its own.
    assert_eq!(
        bot_protection_page("  just a moment...  ", ""),
        Some(Challenge)
    );
    assert_eq!(
        bot_protection_page(
            "",
            "<script>window._cf_chl_opt = {cType: 'managed'};</script>"
        ),
        Some(Challenge)
    );
    assert_eq!(
        bot_protection_page(
            "",
            "<script src=\"/cdn-cgi/challenge-platform/h/g/orchestrate/chl_page/v1\"></script>"
        ),
        Some(Challenge)
    );
    assert_eq!(
        bot_protection_page("Attention Required! | Cloudflare", ""),
        Some(Blocked)
    );
    // A normal provider page behind Cloudflare carries the passive beacon
    // and may mention the phrase in its text; neither is a marker.
    let normal = "<html><head><title>Druid Pulverize Build - Mobalytics</title></head><body>\
                  <p>Just a moment...</p><script src=\"/cdn-cgi/challenge-platform/scripts/jsd/\
                  main.js\"></script></body></html>";
    assert_eq!(
        bot_protection_page("Druid Pulverize Build - Mobalytics", normal),
        None
    );
}

#[test]
fn a_cloudflare_block_page_is_reported_as_blocked_without_a_retry_hint() {
    let session = FakeSession {
        probe_result:
            serde_json::json!({ "title": "Attention Required! | Cloudflare", "html": "" })
                .to_string(),
        ..FakeSession::with_responses(Vec::new())
    };
    let dir = temp_builds_dir("d2core-blocked");

    let error = download_d2core_build("22XD", &dir, &session).expect_err("blocked");

    assert_eq!(error.kind, ImportErrorKind::Blocked);
    assert_eq!(
        error.message,
        "D2Core answered https://www.d2core.com/d4/planner?bd=22XD with a Cloudflare block page: \
         access blocked by bot protection; d4lc does not bypass bot protection"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mobalytics_download_reports_the_challenge_instead_of_a_timeout() {
    let session = FakeSession {
        probe_result: recorded_challenge_probe(),
        ..FakeSession::with_responses(Vec::new())
    };
    let dir = temp_builds_dir("mobalytics-challenge");
    let profiles_dir = temp_builds_dir("mobalytics-challenge-profiles");
    let statuses = std::sync::Mutex::new(Vec::new());

    let error = download_mobalytics_build(
        "https://mobalytics.gg/diablo-4/builds/druid-pulverize",
        &dir,
        &profiles_dir,
        &session,
        &workspace_repo_root(),
        false,
        &|status, _, _| statuses.lock().unwrap().push(status.to_string()),
    )
    .expect_err("a challenge page must fail the download");

    assert!(
        error.message.contains("bot-protection challenge"),
        "{error}"
    );
    assert!(error.message.contains("does not bypass"), "{error}");
    assert!(!error.message.contains("WebDriverWait"), "{error}");
    assert_eq!(
        statuses.into_inner().unwrap(),
        vec!["Downloading https://mobalytics.gg/diablo-4/builds/druid-pulverize.".to_string()]
    );
    assert!(!dir.exists() || std::fs::read_dir(&dir).unwrap().next().is_none());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&profiles_dir);
}

#[test]
fn d2core_download_names_the_challenge_when_no_envelope_arrived() {
    let session = FakeSession {
        probe_result: recorded_challenge_probe(),
        ..FakeSession::with_responses(Vec::new())
    };
    let dir = temp_builds_dir("d2core-challenge");

    let error = download_d2core_build("22XD", &dir, &session).expect_err("challenge must fail");

    assert!(error.message.starts_with("D2Core answered https://www.d2core.com/d4/planner?bd=22XD with a bot-protection challenge"), "{error}");
    assert!(!dir.join("22XD.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn d4builds_download_names_the_challenge_when_the_name_never_appears() {
    let session = ScriptedD4BuildsSession::new(vec![
        (&["outerHTML.slice(0, 65536)"], recorded_challenge_probe()),
        (&["renameBuild"], String::new()),
    ]);
    let dir = temp_d4builds_dir("challenge");

    let error = download_d4builds_build(
        "ancients-barbarian-endgame",
        &dir,
        &session,
        &workspace_repo_root(),
        false,
        &|_, _, _| {},
    )
    .expect_err("challenge must fail");

    assert!(
        error.message.contains("bot-protection challenge"),
        "{error}"
    );
    assert!(!dir.join("ancients-barbarian-endgame.json").exists());
    let _ = std::fs::remove_dir_all(&dir);
}

// -----------------------------------------------------------------------
// Production session bounds (`browser-download` feature).
// -----------------------------------------------------------------------

#[test]
#[cfg(feature = "browser-download")]
fn chrome_binary_override_resolves_paths_and_bare_names() {
    use super::chromium::chrome_binary_override;
    use std::os::unix::fs::PermissionsExt as _;

    let bin = tempfile::TempDir::new().unwrap();
    let browser = bin.path().join("fake-chromium");
    std::fs::write(&browser, b"#!/bin/sh\n").unwrap();
    std::fs::set_permissions(&browser, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(bin.path().join("not-executable"), b"").unwrap();
    let search_path =
        std::env::join_paths([std::path::Path::new("/nonexistent/d4lc"), bin.path()]).unwrap();
    let resolve = |value| chrome_binary_override(value, Some(search_path.as_os_str()));

    // Unset or blank: the standard discovery runs.
    assert_eq!(resolve(None), Ok(None));
    assert_eq!(resolve(Some("   ")), Ok(None));
    // A bare command name is looked up on PATH, as the spawn would.
    assert_eq!(resolve(Some(" fake-chromium ")), Ok(Some(browser.clone())));
    // An explicit path must exist.
    let existing = std::env::current_exe().expect("test binary path");
    let existing_text = existing.display().to_string();
    assert_eq!(resolve(Some(&existing_text)), Ok(Some(existing)));

    for (bad, reason) in [
        ("/nonexistent/d4lc/chromium", "is not an existing file"),
        ("/", "is not an existing file"),
        ("not-executable", "was not found as an executable on PATH"),
        ("missing-browser", "was not found as an executable on PATH"),
    ] {
        let error = resolve(Some(bad)).expect_err(bad);
        assert!(
            matches!(
                error,
                BrowserError::BrowserNotFound {
                    override_set: true,
                    ..
                }
            ),
            "{error:?}"
        );
        // The override is set, so the message names it instead of asking
        // to set it.
        assert_eq!(
            error.to_string(),
            format!("no Chromium/Chrome binary found: D4LC_CHROME_BIN={bad} {reason}")
        );
    }
    assert!(
        chrome_binary_override(Some("fake-chromium"), None).is_err(),
        "no PATH, no lookup"
    );

    // Discovery failure without an override asks to set it.
    let discovery = BrowserError::BrowserNotFound {
        detail: "Could not auto detect a chrome executable".to_string(),
        override_set: false,
    };
    assert_eq!(
        discovery.to_string(),
        "no Chromium/Chrome binary found (set D4LC_CHROME_BIN): Could not auto detect a chrome \
         executable"
    );
}

#[test]
#[cfg(feature = "browser-download")]
fn capture_budget_caps_responses_body_size_and_total_bytes() {
    use super::chromium::{CaptureBudget, CollectionLimits};

    let mut budget = CaptureBudget::new(CollectionLimits {
        deadline: Duration::from_secs(1),
        max_responses: 2,
        max_body_bytes: 10,
        max_total_bytes: 15,
    });
    assert!(budget.admit_response());
    assert!(budget.admit_response());
    assert!(!budget.admit_response());
    assert_eq!(budget.skipped_responses, 1);

    assert!(budget.admit_body(10), "exactly the per-body cap");
    assert!(!budget.admit_body(11), "over the per-body cap");
    assert!(!budget.admit_body(6), "over the total cap");
    assert!(budget.admit_body(5), "exactly the total cap");
    assert!(!budget.admit_body(usize::MAX), "no overflow");
    assert_eq!(budget.skipped_bodies, 3);
    assert_eq!(
        CollectionLimits::PRODUCTION.deadline,
        Duration::from_secs(60)
    );
}

#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only"]
fn browser_collection_ends_at_the_deadline_on_a_page_that_never_goes_quiet() {
    use super::chromium::CollectionLimits;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    // The page polls a small JSON every 100 ms forever and fetches one
    // body over the per-body cap: the inactivity timer alone would never
    // fire.
    const PAGE: &str = "<html><body><script>\
        function get(u){fetch(u).then(function(r){return r.text();});}get('/big.json');\
        setInterval(function(){get('/tick.json?'+Date.now());},100);</script></body></html>";
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let stop = Arc::new(AtomicBool::new(false));
    let server_stop = Arc::clone(&stop);
    let server = std::thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let _ = stream.set_nonblocking(false);
                    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut request = [0; 4096];
                    let read = stream.read(&mut request).unwrap_or(0);
                    let head = String::from_utf8_lossy(&request[..read]);
                    let (kind, body) = if head.starts_with("GET /big.json") {
                        (
                            "application/json",
                            format!("{{\"pad\":\"{}\"}}", "x".repeat(4096)),
                        )
                    } else if head.starts_with("GET /tick.json") {
                        ("application/json", r#"{"tick":1}"#.to_string())
                    } else {
                        ("text/html", PAGE.to_string())
                    };
                    let _ = write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                         Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
    });

    let session = ChromiumSession::launch(BrowserLaunchProfile::D2Core, ImportControl::new())
        .expect("launch browser");
    let limits = CollectionLimits {
        deadline: Duration::from_secs(3),
        max_responses: 5,
        max_body_bytes: 1024,
        max_total_bytes: 1024 * 1024,
    };
    let started = Instant::now();
    let captured = session
        .collect_json_within(
            &format!("{base}/page.html"),
            ".json",
            Duration::from_secs(2),
            limits,
            None,
        )
        .expect("collection completes");
    let elapsed = started.elapsed();
    // Wait-only limits keep listening but fetch no body.
    let wait_only = CollectionLimits {
        max_responses: 0,
        ..limits
    };
    let none = session
        .collect_json_within(
            &format!("{base}/page.html"),
            "",
            Duration::from_secs(2),
            wait_only,
            None,
        )
        .expect("wait-only collection completes");
    assert!(none.is_empty(), "{}", none.len());
    stop.store(true, Ordering::Relaxed);
    server.join().expect("local server");

    assert!(elapsed < Duration::from_secs(8), "took {elapsed:?}");
    assert!(
        elapsed >= Duration::from_secs(3),
        "ended before the deadline: {elapsed:?}"
    );
    assert!(
        !captured.is_empty() && captured.len() <= 5,
        "{}",
        captured.len()
    );
    assert!(captured.iter().all(|response| response.body.len() <= 1024));
    assert!(captured
        .iter()
        .any(|response| response.url.contains("/tick.json")));
}

/// A browser that dies while a page loads fails the collection with a
/// session error, instead of ending it as an empty (successful) capture
/// that the flows would then report as a timeout.
#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only"]
fn a_browser_that_dies_while_loading_fails_the_collection() {
    use super::chromium::CollectionLimits;
    use std::io::Read;
    use std::net::TcpListener;
    use std::time::Instant;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind local server");
    let url = format!(
        "http://{}/build.json",
        listener.local_addr().expect("address")
    );
    let session = ChromiumSession::launch(BrowserLaunchProfile::D2Core, ImportControl::new())
        .expect("launch browser");
    let pid = session.browser_pid().expect("spawned browser pid");
    // The server takes the request, then kills the browser and never
    // answers: only the browser's death can end the load early.
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("browser request");
        let mut request = [0; 4096];
        let _ = stream.read(&mut request);
        let status = std::process::Command::new("kill")
            .args(["-KILL", &pid.to_string()])
            .status()
            .expect("run kill");
        assert!(status.success(), "kill the browser");
        std::thread::sleep(Duration::from_secs(3));
        drop(stream);
    });

    let started = Instant::now();
    let result = session.collect_json_within(
        &url,
        "/build.json",
        Duration::from_secs(20),
        CollectionLimits::PRODUCTION,
        None,
    );
    let elapsed = started.elapsed();
    server.join().expect("local server");
    match result {
        Err(BrowserError::Session(message)) => assert!(
            message.contains("the browser exited unexpectedly"),
            "unexpected message: {message}"
        ),
        other => panic!("a dead browser must fail the collection, got {other:?}"),
    }
    assert!(
        elapsed < Duration::from_secs(5),
        "the failure must not wait for the inactivity timeout: {elapsed:?}"
    );
}

/// A loopback server for the real-browser tests: `/` serves `page`,
/// `/hang` accepts and never answers (a hanging third-party resource),
/// `/data.json` answers `data` after `data_delay`, `/tick.json` answers at
/// once. One thread per connection, so the hanging request blocks nothing.
#[cfg(feature = "browser-download")]
fn loopback_server(
    page: &'static str,
    data: &'static str,
    data_delay: Duration,
) -> (String, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicBool, Ordering};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local server");
    listener.set_nonblocking(true).expect("nonblocking");
    let base = format!("http://{}", listener.local_addr().expect("address"));
    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let server_stop = std::sync::Arc::clone(&stop);
    std::thread::spawn(move || {
        while !server_stop.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let stop = std::sync::Arc::clone(&server_stop);
                    std::thread::spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let mut request = [0; 4096];
                        let read = stream.read(&mut request).unwrap_or(0);
                        let head = String::from_utf8_lossy(&request[..read]).to_string();
                        let (kind, body) = if head.starts_with("GET /hang") {
                            while !stop.load(Ordering::Relaxed) {
                                std::thread::sleep(Duration::from_millis(20));
                            }
                            return;
                        } else if head.starts_with("GET /data.json") {
                            std::thread::sleep(data_delay);
                            ("application/json", data.to_string())
                        } else if head.starts_with("GET /tick.json") {
                            ("application/json", r#"{"tick":1}"#.to_string())
                        } else {
                            ("text/html", page.to_string())
                        };
                        let _ = write!(
                            stream,
                            "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
                             Cache-Control: no-store\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                    });
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => return,
            }
        }
    });
    (base, stop)
}

/// The page keeps polling and never fires its load event (a hanging
/// image), yet the import returns the moment the needed JSON arrived —
/// no inactivity wait, no load wait (BUG-03).
#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only"]
fn browser_returns_the_accepted_response_without_waiting_for_load_or_quiet() {
    const PAGE: &str = "<html><body><img src='/hang'><script>\
        function get(u){fetch(u).then(function(r){return r.text();});}\
        setInterval(function(){get('/tick.json?'+Date.now());},100);\
        setTimeout(function(){get('/data.json');},300);</script></body></html>";
    let (base, stop) = loopback_server(PAGE, r#"{"wanted":"build"}"#, Duration::from_millis(200));
    let session = launch_chrome_session(BrowserLaunchProfile::D2Core, &ImportControl::new())
        .expect("launch browser");
    let started = std::time::Instant::now();
    let accept = |body: &str| body.contains("wanted");
    let captured = session
        .navigate_collecting_json(
            &format!("{base}/"),
            "/data.json",
            Duration::from_secs(10),
            Some(&accept),
        )
        .expect("collection");
    let elapsed = started.elapsed();
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(captured
        .iter()
        .any(|response| response.body.contains("wanted")));
    assert!(
        elapsed < Duration::from_secs(4),
        "returned after {elapsed:?}"
    );
}

/// Every launch flag reaches Chromium as a real switch: chromiumoxide adds
/// the `--` itself, and the Mobalytics user agent decides whether the site
/// serves the build page or a bot-protection challenge (BUG-04).
#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only"]
fn browser_launch_flags_reach_chromium_unmangled() {
    let session = ChromiumSession::launch(mobalytics_launch_profile(false), ImportControl::new())
        .expect("launch browser");
    let pid = session.browser_pid().expect("pid");
    // Chrome may rewrite its argv into one space-joined string, so the
    // switches are matched as words of the whole command line.
    let cmdline = std::fs::read(format!("/proc/{pid}/cmdline")).expect("cmdline");
    let cmdline = String::from_utf8_lossy(&cmdline).replace('\0', " ");
    let words: Vec<&str> = cmdline.split(' ').collect();
    for flag in [
        "--user-agent=Diablo4Companion/1.0",
        "--disable-gpu",
        "--disable-cache",
    ] {
        assert!(words.contains(&flag), "{flag} missing from {cmdline}");
    }
    assert!(cmdline.contains("--window-position=-32000"), "{cmdline}");
    assert!(!cmdline.contains("----"), "mangled flag in {cmdline}");
}

/// Cancel ends a running collection at once, and dropping the cancelled
/// session reaps the browser process.
#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only"]
fn cancel_ends_a_running_collection_and_the_browser() {
    const PAGE: &str = "<html><body><img src='/hang'></body></html>";
    let (base, stop) = loopback_server(PAGE, "{}", Duration::ZERO);
    let control = ImportControl::new();
    let session =
        ChromiumSession::launch(BrowserLaunchProfile::D2Core, control.clone()).expect("launch");
    let pid = session.browser_pid().expect("pid");
    let remote = control.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(500));
        remote.cancel();
    });
    let started = std::time::Instant::now();
    let result = session.navigate_collecting_json(
        &format!("{base}/"),
        "/data.json",
        Duration::from_secs(10),
        Some(&|_: &str| false),
    );
    let elapsed = started.elapsed();
    canceller.join().unwrap();
    assert_eq!(result, Err(BrowserError::Cancelled));
    assert!(elapsed < Duration::from_secs(2), "cancel took {elapsed:?}");
    // Every later call is refused at once.
    assert_eq!(session.evaluate("1"), Err(BrowserError::Cancelled));
    drop(session);
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(
        !std::path::Path::new(&format!("/proc/{pid}")).exists()
            || std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .map(|stat| stat.contains(") Z"))
                .unwrap_or(true),
        "the cancelled session's browser must be gone"
    );
}

/// A document whose load event never fires (a hanging resource) does not
/// fail the import at chromiumoxide's 30 s request timeout: the collection
/// keeps listening and ends on inactivity.
#[test]
#[cfg(feature = "browser-download")]
#[ignore = "requires an installed Chromium with a working sandbox; uses loopback HTTP only (~35 s)"]
fn a_load_event_that_never_fires_is_no_navigation_failure() {
    const PAGE: &str = "<html><body><img src='/hang'><script>\
        fetch('/data.json').then(function(r){return r.text();});</script></body></html>";
    let (base, stop) = loopback_server(PAGE, r#"{"late":1}"#, Duration::ZERO);
    let session = launch_chrome_session(BrowserLaunchProfile::D2Core, &ImportControl::new())
        .expect("launch browser");
    let started = std::time::Instant::now();
    let captured = session
        .navigate_collecting_json(
            &format!("{base}/"),
            "/data.json",
            Duration::from_secs(2),
            None,
        )
        .expect("a hanging load is no failure");
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    assert!(captured
        .iter()
        .any(|response| response.body.contains("late")));
    assert!(
        started.elapsed() < Duration::from_secs(45),
        "{:?}",
        started.elapsed()
    );
}
