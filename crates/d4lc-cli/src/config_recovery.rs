//! `config-recover`: list and explicitly resolve interrupted multi-file
//! configuration changes (`Config/.persistence/txn-*.json`).
//!
//! An interrupted change blocks every later save of its files until an
//! operator decides between completing it and rolling it back. This command
//! is that decision: without an action it only reports what it found and
//! which decisions the files currently allow; with an action it resolves
//! exactly one record. The safety checks (digests, previous/new revision
//! of every file, the journal lock) live in `d4lc_config::persistence`.

use std::path::Path;

use d4lc_config::{interrupted_transactions, InterruptedTransaction, Recovery};

/// Run `config-recover` against `config_root`.
///
/// - `action = None`: list every interrupted record (or only `record`) with
///   its path, state, files and the recoveries that are safely possible.
/// - `action = Some(..)`: recover the record named by `record`, or the only
///   record when there is exactly one, and report the outcome.
///
/// # Errors
///
/// The journal cannot be read; `record` names no interrupted change; an
/// action is requested with no record or with several records and no
/// `record`; or the recovery is refused or fails (nothing is changed when it
/// is refused).
pub(crate) fn config_recover_command(
    config_root: &Path,
    action: Option<Recovery>,
    record: Option<&str>,
) -> Result<String, String> {
    let root = config_root.display();
    let mut records = interrupted_transactions(config_root).map_err(|error| error.to_string())?;
    if let Some(wanted) = record {
        records.retain(|candidate| matches_record(candidate, wanted));
        if records.is_empty() {
            return Err(format!(
                "no interrupted configuration change {wanted} under {root}"
            ));
        }
    }
    let Some(action) = action else {
        return Ok(listing(config_root, &records));
    };
    if records.len() > 1 {
        let ids: Vec<&str> = records.iter().map(InterruptedTransaction::id).collect();
        return Err(format!(
            "{} interrupted configuration changes under {root} ({}); name the one to {action} with --record <id>",
            records.len(),
            ids.join(", ")
        ));
    }
    let Some(selected) = records.pop() else {
        return Err(format!(
            "no interrupted configuration change to {action} under {root}"
        ));
    };
    let id = selected.id().to_string();
    if let Err(reason) = selected.check(action) {
        return Err(format!(
            "--{action} is not safely possible for {id}: {reason}; nothing was changed"
        ));
    }
    let outcome = selected
        .recover(action)
        .map_err(|error| format!("--{action} for {id} failed: {error}"))?;
    let remaining = interrupted_transactions(config_root)
        .map_err(|error| error.to_string())?
        .len();
    let after = match action {
        Recovery::Complete => "every file now holds the new content of the change",
        Recovery::Rollback => "every file holds its content from before the change",
    };
    let saves = if remaining == 0 {
        "configuration saves are no longer blocked".to_string()
    } else {
        format!("{remaining} more interrupted change(s) remain; run config-recover again")
    };
    Ok(format!(
        "recovered {id} under {root} with --{action}: {}\n{after}; {saves}\n",
        outcome.files.join(", ")
    ))
}

/// A record matches by its id (`txn-…`) or its file name (`txn-….json`).
fn matches_record(record: &InterruptedTransaction, wanted: &str) -> bool {
    record.id() == wanted
        || record
            .record_path()
            .file_name()
            .is_some_and(|name| name == wanted)
}

fn listing(config_root: &Path, records: &[InterruptedTransaction]) -> String {
    let root = config_root.display();
    if records.is_empty() {
        return format!("no interrupted configuration changes under {root}; nothing to recover\n");
    }
    let mut text = format!(
        "interrupted configuration changes under {root}: {}\n",
        records.len()
    );
    for record in records {
        text.push_str(&format!("record {}\n", record.id()));
        text.push_str(&format!("  path: {}\n", record.record_path().display()));
        text.push_str(&format!("  state: {}\n", record.state()));
        if !record.detail().is_empty() {
            text.push_str(&format!(
                "  unreadable: {}\n  neither --complete nor --rollback is possible; inspect the \
                 record and the configuration files by hand\n",
                record.detail()
            ));
            continue;
        }
        text.push_str(&format!("  files: {}\n", record.files().join(", ")));
        for (action, effect) in [
            (Recovery::Complete, "every file gets the new content"),
            (
                Recovery::Rollback,
                "every file gets back its content from before the change",
            ),
        ] {
            match record.check(action) {
                Ok(()) => text.push_str(&format!("  --{action}: possible ({effect})\n")),
                Err(reason) => text.push_str(&format!("  --{action}: not possible: {reason}\n")),
            }
        }
        if record.is_untouched() {
            text.push_str(
                "  no file was replaced yet and the change was never reported as saved; the app \
                 discards it at its next start (the same as --rollback)\n",
            );
        }
    }
    let first = records[0].id();
    text.push_str(&format!(
        "resolve with: d4lc-debug config-recover --complete --record {first}  or  \
         d4lc-debug config-recover --rollback --record {first}\n"
    ));
    text
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use d4lc_config::persistence::{prepare_transaction, FileWrite};
    use std::fs;
    use std::path::PathBuf;

    const A: &str = "Config/AffixPresets-v2.json";
    const B: &str = "Config/Settings.json";

    fn root(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let root = std::env::temp_dir().join(format!("d4lc-cli-config-recover-{name}-{nanos}"));
        fs::create_dir_all(root.join("Config")).expect("create root");
        fs::write(root.join(A), "old a").expect("seed a");
        fs::write(root.join(B), "old b").expect("seed b");
        root
    }

    fn writes() -> [FileWrite<'static>; 2] {
        [
            FileWrite {
                relative_path: A,
                contents: "new a",
            },
            FileWrite {
                relative_path: B,
                contents: "new b",
            },
        ]
    }

    /// The first file is replaced and the second replacement fails: a
    /// partial change whose record stays behind.
    fn interrupt_between_replacements(root: &Path) {
        let prepared = prepare_transaction(root, &writes()).expect("prepare");
        fs::remove_file(root.join(B)).expect("remove b");
        fs::create_dir(root.join(B)).expect("obstruct b");
        prepared.commit().expect_err("second replacement fails");
        fs::remove_dir(root.join(B)).expect("clear obstruction");
        fs::write(root.join(B), "old b").expect("restore b as before");
    }

    /// What a kill after the record and before the first replacement
    /// leaves on disk.
    fn interrupt_before_any_replacement(root: &Path) {
        drop(prepare_transaction(root, &writes()).expect("prepare"));
    }

    fn read(root: &Path, relative: &str) -> String {
        fs::read_to_string(root.join(relative)).expect("read file")
    }

    #[test]
    fn listing_without_records_says_there_is_nothing_to_recover() {
        let root = root("empty");
        let text = config_recover_command(&root, None, None).expect("list");
        assert!(text.contains("nothing to recover"), "{text}");
        let error = config_recover_command(&root, Some(Recovery::Rollback), None)
            .expect_err("no record to recover");
        assert!(
            error.contains("no interrupted configuration change"),
            "{error}"
        );
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn listing_names_path_state_files_and_safe_options() {
        let root = root("listing");
        interrupt_between_replacements(&root);
        let id = interrupted_transactions(&root).expect("detect")[0]
            .id()
            .to_string();
        let text = config_recover_command(&root, None, None).expect("list");
        for expected in [
            format!("record {id}"),
            "  path: ".to_string(),
            "  state: prepared".to_string(),
            format!("  files: {A}, {B}"),
            "  --complete: possible".to_string(),
            "  --rollback: possible".to_string(),
            format!("d4lc-debug config-recover --complete --record {id}"),
        ] {
            assert!(text.contains(&expected), "missing {expected:?} in\n{text}");
        }
        assert!(!text.contains("discards it at its next start"), "{text}");
        // Listing changes nothing.
        assert_eq!(read(&root, A), "new a");
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn listing_explains_why_an_option_is_not_possible() {
        let root = root("refused_option");
        interrupt_between_replacements(&root);
        fs::write(root.join(A), "edited by hand").expect("edit a");
        let text = config_recover_command(&root, None, None).expect("list");
        assert!(text.contains("--complete: not possible: "), "{text}");
        assert!(text.contains("--rollback: not possible: "), "{text}");
        let error =
            config_recover_command(&root, Some(Recovery::Rollback), None).expect_err("refused");
        assert!(error.contains("nothing was changed"), "{error}");
        assert_eq!(read(&root, A), "edited by hand");
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn rollback_restores_the_previous_revision_and_unblocks_saves() {
        let root = root("rollback");
        interrupt_between_replacements(&root);
        let text = config_recover_command(&root, Some(Recovery::Rollback), None).expect("rollback");
        assert!(text.contains("no longer blocked"), "{text}");
        assert_eq!(read(&root, A), "old a");
        assert_eq!(read(&root, B), "old b");
        prepare_transaction(&root, &writes())
            .and_then(|prepared| prepared.commit())
            .expect("a new save succeeds");
        assert_eq!(read(&root, B), "new b");
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn complete_applies_the_new_revision_and_unblocks_saves() {
        let root = root("complete");
        interrupt_between_replacements(&root);
        config_recover_command(&root, Some(Recovery::Complete), None).expect("complete");
        assert_eq!(read(&root, A), "new a");
        assert_eq!(read(&root, B), "new b");
        assert!(interrupted_transactions(&root).expect("list").is_empty());
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn an_untouched_change_is_marked_as_such() {
        let root = root("untouched");
        interrupt_before_any_replacement(&root);
        let text = config_recover_command(&root, None, None).expect("list");
        assert!(text.contains("discards it at its next start"), "{text}");
        config_recover_command(&root, Some(Recovery::Rollback), None).expect("rollback");
        assert_eq!(read(&root, A), "old a");
        assert_eq!(read(&root, B), "old b");
        fs::remove_dir_all(root).expect("clean");
    }

    #[test]
    fn several_records_need_an_explicit_record_id() {
        let root = root("several");
        interrupt_before_any_replacement(&root);
        // A second record beside the real one. A copy under a foreign name
        // is unreadable (its paths belong to another transaction) but still
        // makes the choice ambiguous.
        let journal = root.join(d4lc_config::persistence::JOURNAL_DIR);
        let first = interrupted_transactions(&root).expect("detect")[0]
            .record_path()
            .to_path_buf();
        fs::copy(&first, journal.join("txn-0-second.json")).expect("second record");
        let error =
            config_recover_command(&root, Some(Recovery::Rollback), None).expect_err("ambiguous");
        assert!(error.contains("--record"), "{error}");
        let error =
            config_recover_command(&root, None, Some("txn-unknown")).expect_err("unknown record");
        assert!(error.contains("txn-unknown"), "{error}");

        let id = interrupted_transactions(&root)
            .expect("detect")
            .iter()
            .find(|record| record.detail().is_empty())
            .expect("the real record is readable")
            .id()
            .to_string();
        let text = config_recover_command(&root, None, Some(&id)).expect("filtered list");
        assert!(text.contains(": 1\n"), "{text}");
        let text =
            config_recover_command(&root, Some(Recovery::Rollback), Some(&id)).expect("rollback");
        assert!(text.contains("1 more interrupted change"), "{text}");
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        fs::remove_dir_all(root).expect("clean");
    }

    /// The `config-recover` command reads the desktop app's config root
    /// (here `D4LC_CONFIG_ROOT`), not the asset root, and maps the flags.
    #[test]
    fn the_cli_command_resolves_the_config_root_and_maps_the_flags() {
        let root = root("cli");
        interrupt_between_replacements(&root);
        let assets = std::env::temp_dir();
        let invocation = crate::roots::Invocation {
            cwd: assets.clone(),
            config_root: Some(root.clone()),
            ..crate::roots::Invocation::default()
        };
        let run = |args: &[&str]| crate::run(args.iter().copied(), &invocation);

        let listing = run(&["config-recover"]).expect("list");
        assert!(listing.contains(&root.display().to_string()), "{listing}");
        assert!(listing.contains(A), "{listing}");
        run(&["config-recover", "--complete", "--rollback"]).expect_err("conflicting actions");
        assert_eq!(
            read(&root, A),
            "new a",
            "nothing changes on a rejected call"
        );

        let text = run(&["config-recover", "--rollback"]).expect("rollback");
        assert!(!text.is_empty());
        assert_eq!(read(&root, A), "old a");
        assert_eq!(read(&root, B), "old b");
        assert!(interrupted_transactions(&root).expect("list").is_empty());
        fs::remove_dir_all(root).expect("clean");
    }
}
