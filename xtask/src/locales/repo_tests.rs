//! Checks over the generated catalogues as checked into the repository.

use super::*;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn upstream_present() -> bool {
    let present = root().join(RESX_DIR).join("Resources.resx").is_file();
    if !present {
        eprintln!(
            "skipping: upstream submodule not initialised (scripts/init_upstream_submodule.sh)"
        );
    }
    present
}

#[test]
fn generation_is_deterministic() {
    if !upstream_present() {
        return;
    }
    let first = generate(&root()).unwrap();
    let second = generate(&root()).unwrap();
    assert_eq!(first.outputs.len(), second.outputs.len());
    for (a, b) in first.outputs.iter().zip(&second.outputs) {
        assert_eq!(a.path, b.path);
        assert_eq!(a.contents, b.contents);
    }
}

#[test]
fn checked_in_catalogues_match_the_importer() {
    if !upstream_present() {
        return;
    }
    run(&root(), true).unwrap();
}

#[test]
fn english_catalogue_maps_every_msgid_to_itself() {
    let en = fs::read_to_string(
        root()
            .join(TRANSLATIONS_DIR)
            .join("en-US/LC_MESSAGES")
            .join(format!("{DOMAIN}.po")),
    )
    .unwrap();
    let mut rows = 0;
    for block in en.split("\n\n").skip(1) {
        let msgid = block
            .lines()
            .find_map(|line| line.strip_prefix("msgid "))
            .unwrap();
        let msgstr = block
            .lines()
            .find_map(|line| line.strip_prefix("msgstr "))
            .unwrap();
        assert_eq!(msgid, msgstr);
        rows += 1;
    }
    assert!(rows > 0, "the English catalogue is empty");
}

/// Every `@tr` in the shipped UI resolves: either upstream owns the string
/// (it is in the English catalogue) or it is a documented port-only gap.
#[test]
fn every_ui_string_is_in_the_catalogue_or_the_gap_list() {
    let read = |relative: &str| fs::read_to_string(root().join(relative)).unwrap();
    let calls = |text: &str| -> BTreeSet<TrCall> {
        text.lines()
            .filter(|line| {
                !line.starts_with('#') && !line.is_empty() && !line.starts_with("msgstr")
            })
            .flat_map(|line| extract_tr_calls(&format!("@tr({line})"), "list").unwrap())
            .collect()
    };
    let catalogue = read(&format!("{TRANSLATIONS_DIR}/en-US/LC_MESSAGES/{DOMAIN}.po"));
    let known: BTreeSet<String> = catalogue
        .lines()
        .filter_map(|line| line.strip_prefix("msgid "))
        .filter(|quoted| *quoted != "\"\"")
        .map(|quoted| {
            extract_tr_calls(&format!("@tr({quoted})"), "en.po")
                .unwrap()
                .remove(0)
                .msgid
        })
        .collect();
    let gap = calls(&read(PORT_ONLY));
    let used = ui_calls(&root()).unwrap();
    let unknown: Vec<&TrCall> = used
        .iter()
        .filter(|call| {
            !(call.context.is_none() && known.contains(&call.msgid)) && !gap.contains(*call)
        })
        .collect();
    assert!(
        unknown.is_empty(),
        "@tr strings missing from the catalogue and the gap list (run `cargo xtask import-upstream-locales`): {unknown:?}"
    );
    let stale: Vec<&TrCall> = gap.iter().filter(|call| !used.contains(*call)).collect();
    assert!(
        stale.is_empty(),
        "gap list entries no longer used in the UI: {stale:?}"
    );
    let upstream_owned: Vec<&TrCall> = gap
        .iter()
        .filter(|call| call.context.is_none() && known.contains(&call.msgid))
        .collect();
    assert!(
        upstream_owned.is_empty(),
        "gap list entries that upstream does own: {upstream_owned:?}"
    );
}

/// All upstream `.xaml` and `.cs` sources, concatenated: where the resource
/// keys are bound. The resx files and the generated designer are not bindings.
fn upstream_sources() -> String {
    let mut text = String::new();
    let mut pending = vec![root().join("upstream/Diablo4Companion")];
    while let Some(dir) = pending.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if path.is_dir() {
                if !matches!(
                    name.as_str(),
                    ".git" | "bin" | "obj" | "D4Companion.Localization"
                ) {
                    pending.push(path);
                }
            } else if (name.ends_with(".xaml") || name.ends_with(".cs"))
                && !name.ends_with("Designer.cs")
            {
                text.push_str(&String::from_utf8_lossy(&fs::read(&path).unwrap()));
            }
        }
    }
    text
}

/// A key in the map is a claim that a control upstream binds it. A key that no
/// view or code binds would hand its translation to a string upstream never
/// translates (`rsCapOverlay`, `rsCapSeasonal`).
#[test]
fn every_consumed_key_is_bound_somewhere_upstream() {
    if !upstream_present() {
        return;
    }
    let sources = upstream_sources();
    let keys = parse_key_map(&fs::read_to_string(root().join(KEY_MAP)).unwrap()).unwrap();
    let unbound: Vec<&String> = keys
        .iter()
        .filter(|key| !sources.contains(key.as_str()))
        .collect();
    assert!(
        unbound.is_empty(),
        "keys in {KEY_MAP} that no upstream .xaml or .cs binds: {unbound:?}"
    );
}

/// zh-CN is translated in full: every row has a translation and the rows the
/// port wrote (not upstream) say so.
#[test]
fn zh_cn_is_complete_and_marks_the_port_translations() {
    let po = fs::read_to_string(
        root()
            .join(TRANSLATIONS_DIR)
            .join("zh-CN/LC_MESSAGES")
            .join(format!("{DOMAIN}.po")),
    )
    .unwrap();
    let mut port_rows = 0;
    for block in po.split("\n\n").skip(1) {
        assert!(
            !block.lines().any(|line| line == "msgstr \"\""),
            "untranslated zh-CN row: {block}"
        );
        let marked = block
            .lines()
            .any(|line| line == format!("#. {PORT_COMMENT}"));
        let upstream_keyed = block.lines().any(|line| line.starts_with("#. rs"));
        if marked {
            port_rows += 1;
        } else {
            assert!(
                upstream_keyed,
                "a row without a key must be marked as a port translation: {block}"
            );
        }
    }
    assert!(port_rows > 0);
}
