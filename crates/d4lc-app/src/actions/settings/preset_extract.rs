//! Community preset zip extraction.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

use super::preset_download::{
    sanitize_preset_file_name, PresetDownloadOutcome, PRESET_ARCHIVE_MAX_BYTES,
};
use super::preset_install::{
    entry_type, install_preset_tree, io_error, no_hook, trusted_images_dir, InstallStep,
    PresetInstallError, StepHook,
};

// ── Community preset extraction ───────────────────────────
//
// Upstream evidence:
//   - `SettingsViewModel.cs:313-323`
//     (`HandleDownloadSystemPresetCompletedMessage`) triggers
//     `SystemPresetManager.ExtractSystemPreset` right after the download
//   completes.
//   - `SystemPresetManager.cs:107-118` extracts with
//     `ZipFile.ExtractToDirectory($".\\Images\\{fileName}", ".\\Images",
//     overwriteFiles: true)`: the shipped archives carry a single
//     `{preset}/` root directory, so the extraction result is the
//     extracted preset tree `Images\{preset}\` next to the zip.
//
// Linux adaptation: the destination is `config_root/Images/` (the same
// `PRESETS_IMAGES_SUBDIR` the download writes into) and the zip's single
// root directory names the extracted preset
// (`config_root/Images/{preset}`). The overwrite policy is upstream's:
// every file of the archive replaces the file at its path and files that
// are not in the archive are kept. Hardening deltas against the upstream
// call:
//   - entry paths are validated before anything is written (plain
//     components only, one non-hidden root directory, no backslash or
//     control characters, bounded length and depth, no symlink entries);
//   - entry count, per-file and total sizes are bounded, counting the bytes
//     actually decompressed rather than trusting the archive metadata;
//   - the archive is applied to a staged copy of the preset tree that
//     replaces the live tree only when complete (`preset_install`), so a
//     failed import leaves the previous tree unchanged.

/// Limits on a preset archive. The published archives (all ten measured)
/// have 12–43 entries, at most 89 011 bytes uncompressed in total, files of
/// at most 16 212 bytes, names of at most 61 bytes and depth 3.
pub(crate) const MAX_ARCHIVE_ENTRIES: usize = 1024;
pub(crate) const MAX_EXTRACTED_FILE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_EXTRACTED_TOTAL_BYTES: u64 = 64 * 1024 * 1024;
pub(crate) const MAX_ENTRY_PATH_BYTES: usize = 255;
pub(crate) const MAX_ENTRY_PATH_DEPTH: usize = 8;

/// Unix file-type bits of a zip entry's external attributes.
const S_IFMT: u32 = 0o170_000;
const S_IFLNK: u32 = 0o120_000;

/// Outcome of a successful preset extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetExtractionOutcome {
    /// The archive's single root directory = the extracted preset name.
    pub preset_name: String,
    /// Absolute path of the extracted preset directory
    /// (`config_root/Images/{preset_name}`).
    pub preset_dir: PathBuf,
    /// Number of regular files written.
    pub files: usize,
}

/// Download + extraction result of the wired
/// [`run_preset_download_job`](super::preset_download_job::run_preset_download_job) flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetDownloadResult {
    pub download: PresetDownloadOutcome,
    pub extraction: PresetExtractionOutcome,
}

/// Extract an already-downloaded preset zip
/// (`config_root/Images/{file_name}`) into
/// `config_root/Images/{root_dir}` where `root_dir` is the archive's
/// single top-level directory (upstream `ExtractSystemPreset` parity —
/// see the module comment above for the hardening deltas).
///
/// # Errors
///
/// Fails when the zip is unreadable, corrupt, empty, larger than
/// [`PRESET_ARCHIVE_MAX_BYTES`], mixes root directories, carries an unsafe
/// or symlink entry, exceeds an extraction limit, would write through a
/// symlink, or when a filesystem operation fails. In every case the previous
/// `Images/{preset}` tree is unchanged, except after
/// [`PresetInstallError::RollbackFailed`], whose tree is restored by the
/// next install or startup recovery.
pub fn extract_preset(
    config_root: &Path,
    file_name: &str,
) -> Result<PresetExtractionOutcome, PresetInstallError> {
    extract_preset_with_hook(config_root, file_name, &no_hook)
}

fn extract_preset_with_hook(
    config_root: &Path,
    file_name: &str,
    hook: StepHook<'_>,
) -> Result<PresetExtractionOutcome, PresetInstallError> {
    let sanitized = sanitize_preset_file_name(file_name).map_err(PresetInstallError::UnsafePath)?;
    let images_dir = trusted_images_dir(config_root, false)?;
    let mut archive = open_archive(&images_dir.join(&sanitized), &sanitized)?;
    let root_dir = validate_entries(&sanitized, &mut archive)?;
    let files = install_preset_tree(
        &images_dir,
        &root_dir,
        &mut |staged| extract_archive_entries(&mut archive, &root_dir, staged, hook),
        hook,
    )?;
    let preset_dir = images_dir.join(&root_dir);
    eprintln!(
        "[d4lc] extract_preset: '{}' -> {} ({} files)",
        sanitized,
        preset_dir.display(),
        files
    );
    Ok(PresetExtractionOutcome {
        preset_name: root_dir,
        preset_dir,
        files,
    })
}

type Archive = zip::ZipArchive<std::io::BufReader<std::fs::File>>;

/// Open the archive at `zip_path` (not through a symlink) and check its
/// on-disk size and entry count before reading entries. The symlink check
/// and the open are two steps (`O_NOFOLLOW` would need `libc`); only the
/// owner of the config root can race them, which the threat model in
/// `preset_install` accepts.
fn open_archive(zip_path: &Path, archive_name: &str) -> Result<Archive, PresetInstallError> {
    if entry_type(zip_path)?.is_some_and(|kind| kind.is_symlink()) {
        return Err(PresetInstallError::SymlinkRefused(zip_path.to_path_buf()));
    }
    let file =
        std::fs::File::open(zip_path).map_err(io_error("open downloaded preset", zip_path))?;
    let metadata = file
        .metadata()
        .map_err(io_error("inspect downloaded preset", zip_path))?;
    if !metadata.is_file() {
        return Err(PresetInstallError::PathConflict(zip_path.to_path_buf()));
    }
    if metadata.len() > PRESET_ARCHIVE_MAX_BYTES {
        return Err(PresetInstallError::LimitExceeded {
            what: "archive size (bytes)",
            limit: PRESET_ARCHIVE_MAX_BYTES,
        });
    }
    let archive = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| {
        PresetInstallError::InvalidArchive(format!(
            "downloaded preset '{archive_name}' is not a valid zip archive: {error}"
        ))
    })?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(PresetInstallError::LimitExceeded {
            what: "entry count",
            limit: MAX_ARCHIVE_ENTRIES as u64,
        });
    }
    Ok(archive)
}

/// Whether every component of `path` is a plain name: no `..`, `.`, root or
/// drive prefix. An entry such as `P/../Q/x.png` stays inside the extraction
/// directory but would land outside the preset it claims to belong to.
fn has_only_normal_components(path: &Path) -> bool {
    // `Path::components` folds an inner `.` away, so the raw segments are
    // checked too.
    path.components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
        && !path
            .to_string_lossy()
            .split('/')
            .any(|segment| segment == "." || segment == "..")
}

/// Validate every entry (path shape, limits, no symlink entries), that all
/// of them live under exactly one non-hidden top-level directory, and that
/// at least one of them is a file below it. Returns that root directory
/// name. Reads only the central directory.
fn validate_entries(
    archive_name: &str,
    archive: &mut Archive,
) -> Result<String, PresetInstallError> {
    let unsafe_entry = |name: &str, why: &str| {
        PresetInstallError::UnsafePath(format!(
            "preset archive '{archive_name}' contains an unsafe entry path '{name}' ({why})"
        ))
    };
    let mut root_dir: Option<String> = None;
    let mut files = 0usize;
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).map_err(|error| {
            PresetInstallError::InvalidArchive(format!(
                "preset archive '{archive_name}' entry {index} is unreadable: {error}"
            ))
        })?;
        let name = entry.name();
        if name.len() > MAX_ENTRY_PATH_BYTES {
            return Err(PresetInstallError::LimitExceeded {
                what: "entry path length (bytes)",
                limit: MAX_ENTRY_PATH_BYTES as u64,
            });
        }
        if name.chars().any(|c| c.is_control() || c == '\\') {
            return Err(unsafe_entry(name, "backslash or control character"));
        }
        let path = Path::new(name);
        if name.starts_with('/') || !has_only_normal_components(path) {
            return Err(unsafe_entry(name, "not a plain relative path"));
        }
        if path.components().count() > MAX_ENTRY_PATH_DEPTH {
            return Err(PresetInstallError::LimitExceeded {
                what: "entry path depth",
                limit: MAX_ENTRY_PATH_DEPTH as u64,
            });
        }
        if entry
            .unix_mode()
            .is_some_and(|mode| mode & S_IFMT == S_IFLNK)
        {
            return Err(unsafe_entry(name, "symlink entry"));
        }
        let first = name.split('/').next().unwrap_or_default();
        if first.starts_with('.') {
            return Err(unsafe_entry(name, "hidden root directory"));
        }
        if !entry.is_dir() {
            if !name.contains('/') {
                return Err(PresetInstallError::InvalidArchive(format!(
                    "preset archive '{archive_name}' has a file '{name}' instead of a root \
                     directory"
                )));
            }
            files += 1;
        }
        match &root_dir {
            None => root_dir = Some(first.to_string()),
            Some(existing) if existing != first => {
                return Err(PresetInstallError::InvalidArchive(format!(
                    "preset archive '{archive_name}' mixes multiple root directories \
                     ('{existing}', '{first}')"
                )));
            }
            Some(_) => {}
        }
    }
    let root_dir = root_dir.ok_or_else(|| {
        PresetInstallError::InvalidArchive(format!(
            "preset archive '{archive_name}' contains no entries"
        ))
    })?;
    if files == 0 {
        return Err(PresetInstallError::InvalidArchive(format!(
            "preset archive '{archive_name}' contains no files"
        )));
    }
    Ok(root_dir)
}

/// Apply every archive entry (already validated by [`validate_entries`]) to
/// the staged tree `staged`, below the archive's root directory. Returns the
/// number of files written. Sizes are enforced on the decompressed bytes
/// actually read, not on the entry headers.
fn extract_archive_entries(
    archive: &mut Archive,
    root_dir: &str,
    staged: &Path,
    hook: StepHook<'_>,
) -> Result<usize, PresetInstallError> {
    let mut files = 0usize;
    let mut total = 0u64;
    let mut buffer = vec![0_u8; 64 * 1024];
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| {
            PresetInstallError::InvalidArchive(format!(
                "preset archive entry {index} is unreadable: {error}"
            ))
        })?;
        let name = entry.name().to_string();
        let relative = Path::new(name.trim_end_matches('/'))
            .strip_prefix(root_dir)
            .map_err(|_| PresetInstallError::UnsafePath(format!("entry '{name}' left its root")))?
            .to_path_buf();
        if relative.as_os_str().is_empty() {
            continue; // the root directory entry itself
        }
        if entry.is_dir() {
            create_staged_dirs(staged, &relative)?;
            continue;
        }
        if entry.size() > MAX_EXTRACTED_FILE_BYTES {
            return Err(PresetInstallError::LimitExceeded {
                what: "extracted file size (bytes)",
                limit: MAX_EXTRACTED_FILE_BYTES,
            });
        }
        let path = prepare_file_slot(staged, &relative)?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(io_error("create extracted file", &path))?;
        let mut written = 0u64;
        loop {
            let read = match entry.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => read,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    return Err(PresetInstallError::InvalidArchive(format!(
                        "preset archive entry '{name}' cannot be decompressed: {error}"
                    )));
                }
            };
            written += read as u64;
            if written > MAX_EXTRACTED_FILE_BYTES {
                return Err(PresetInstallError::LimitExceeded {
                    what: "extracted file size (bytes)",
                    limit: MAX_EXTRACTED_FILE_BYTES,
                });
            }
            if total + written > MAX_EXTRACTED_TOTAL_BYTES {
                return Err(PresetInstallError::LimitExceeded {
                    what: "total extracted size (bytes)",
                    limit: MAX_EXTRACTED_TOTAL_BYTES,
                });
            }
            out.write_all(&buffer[..read])
                .map_err(io_error("write extracted file", &path))?;
        }
        total += written;
        files += 1;
        hook(InstallStep::FileStaged(files)).map_err(io_error("stage extracted file", &path))?;
    }
    Ok(files)
}

/// Create the directories of `relative` below `staged` one component at a
/// time, refusing to descend through a symlink or into a file. Returns the
/// full path.
fn create_staged_dirs(staged: &Path, relative: &Path) -> Result<PathBuf, PresetInstallError> {
    let mut current = staged.to_path_buf();
    for component in relative.components() {
        current.push(component);
        match entry_type(&current)? {
            None => std::fs::create_dir(&current)
                .map_err(io_error("create extracted directory", &current))?,
            Some(kind) if kind.is_symlink() => {
                return Err(PresetInstallError::SymlinkRefused(current));
            }
            Some(kind) if kind.is_dir() => {}
            Some(_) => return Err(PresetInstallError::PathConflict(current)),
        }
    }
    Ok(current)
}

/// Make `staged/relative` ready for a `create_new` file: parents created
/// (never through a symlink), an existing file or symlink there removed
/// (upstream overwrite; a link is replaced, not followed), a directory there
/// refused.
fn prepare_file_slot(staged: &Path, relative: &Path) -> Result<PathBuf, PresetInstallError> {
    if let Some(parent) = relative.parent() {
        create_staged_dirs(staged, parent)?;
    }
    let path = staged.join(relative);
    match entry_type(&path)? {
        None => {}
        Some(kind) if kind.is_dir() => return Err(PresetInstallError::PathConflict(path)),
        Some(_) => {
            std::fs::remove_file(&path).map_err(io_error("replace extracted file", &path))?
        }
    }
    Ok(path)
}

// ── Tests for preset zip extraction ───────────────────────
//
// Decision points: traversal entries must never write outside
// `Images/`, a corrupt archive must fail with a structured error, and
// the overwrite policy (per file, files not in the archive are kept) must
// never leave a half-written file. The oracle is
// the archive content / the file tree read back from disk.
#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::bool_assert_comparison
)]
mod tests {
    use super::*;
    use crate::actions::settings::preset_download::PRESETS_IMAGES_SUBDIR;
    use crate::actions::settings::preset_install::tests::snapshot;
    use crate::actions::settings::preset_install::INSTALL_WORK_DIR;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// The production function with its error rendered as the UI sees it.
    fn extract_preset(
        config_root: &Path,
        file_name: &str,
    ) -> Result<PresetExtractionOutcome, String> {
        super::extract_preset(config_root, file_name).map_err(|error| error.to_string())
    }

    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_tmp_root(label: &str) -> PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let pid = std::process::id();
        std::env::temp_dir().join(format!("d4lc-preset-extract-{label}-{pid}-{n}"))
    }

    /// Build a zip at `root/Images/{name}` from `(entry name, bytes)`
    /// pairs (same shape the shipped upstream archives have: a single
    /// `{preset}/` root directory).
    fn write_preset_zip(root: &Path, name: &str, entries: &[(&str, &[u8])]) {
        use zip::write::SimpleFileOptions;
        let images_dir = root.join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).expect("create images dir");
        let path = images_dir.join(name);
        let file = std::fs::File::create(&path).expect("create zip file");
        let mut writer = zip::ZipWriter::new(file);
        for (entry_name, contents) in entries {
            writer
                .start_file(*entry_name, SimpleFileOptions::default())
                .unwrap_or_else(|error| panic!("start zip member {entry_name}: {error}"));
            writer
                .write_all(contents)
                .unwrap_or_else(|error| panic!("write zip member {entry_name}: {error}"));
        }
        writer
            .finish()
            .unwrap_or_else(|error| panic!("finish zip {}: {error}", path.display()));
    }

    fn preset_config_body(tooltip_width: u32) -> String {
        format!(
            "{{\"AffixAreaHeightOffsetTop\": 8, \
             \"AffixAreaHeightOffsetBottom\": 8, \
             \"AffixAspectAreaWidthOffset\": 18, \
             \"AspectAreaHeightOffsetTop\": 8, \
             \"ParagonLeftOffsetCollapsed\": -200, \
             \"ParagonNodeSize\": 29, \
             \"ParagonNodeSizeCollapsed\": 16, \
             \"ParagonTopOffsetCollapsed\": -50, \
             \"ThresholdMin\": 70, \
             \"ThresholdMax\": 255, \
             \"TooltipHeightType\": 200, \
             \"TooltipWidth\": {tooltip_width}}}"
        )
    }

    #[test]
    fn extract_preset_unpacks_the_single_root_directory_under_images() {
        let root = unique_tmp_root("happy");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");
        write_preset_zip(
            &root,
            "9000p_TEST.zip",
            &[
                ("9000p_TEST/config.json", preset_config_body(375).as_bytes()),
                ("9000p_TEST/tooltip_kb_all.png", b"png-bytes".as_slice()),
            ],
        );

        let outcome = extract_preset(&root, "9000p_TEST.zip").expect("extraction must succeed");
        assert_eq!(outcome.preset_name, "9000p_TEST");
        assert_eq!(
            outcome.preset_dir,
            root.join(PRESETS_IMAGES_SUBDIR).join("9000p_TEST")
        );
        assert_eq!(outcome.files, 2);
        let config = std::fs::read_to_string(outcome.preset_dir.join("config.json"))
            .expect("extracted config");
        assert!(config.contains("\"TooltipWidth\": 375"), "got: {config}");
        assert!(
            outcome.preset_dir.join("tooltip_kb_all.png").is_file(),
            "template files must be extracted"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_preset_overwrites_per_file_and_keeps_files_not_in_the_archive() {
        // `ZipFile.ExtractToDirectory(..., overwriteFiles: true)`
        // (SystemPresetManager.cs:116): files of the archive replace the
        // files at their paths; files that are not in the archive stay.
        let root = unique_tmp_root("overwrite");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");

        write_preset_zip(
            &root,
            "9000p_TEST.zip",
            &[
                ("9000p_TEST/config.json", preset_config_body(111).as_bytes()),
                ("9000p_TEST/kept_template.png", b"kept".as_slice()),
                ("9000p_TEST/Tooltips/tooltip_kb_all.png", b"old".as_slice()),
            ],
        );
        extract_preset(&root, "9000p_TEST.zip").expect("first extraction");
        let preset_dir = root.join(PRESETS_IMAGES_SUBDIR).join("9000p_TEST");
        std::fs::write(preset_dir.join("user_note.txt"), b"mine").expect("user file");

        write_preset_zip(
            &root,
            "9000p_TEST.zip",
            &[
                ("9000p_TEST/config.json", preset_config_body(222).as_bytes()),
                ("9000p_TEST/Tooltips/tooltip_kb_all.png", b"new".as_slice()),
                ("9000p_TEST/Tooltips/added.png", b"added".as_slice()),
            ],
        );
        let outcome = extract_preset(&root, "9000p_TEST.zip").expect("re-extraction");

        assert_eq!(outcome.files, 3);
        let config = std::fs::read_to_string(preset_dir.join("config.json")).expect("config");
        assert!(config.contains("\"TooltipWidth\": 222"), "got: {config}");
        assert_eq!(
            std::fs::read(preset_dir.join("Tooltips/tooltip_kb_all.png")).expect("overwritten"),
            b"new"
        );
        assert_eq!(
            std::fs::read(preset_dir.join("Tooltips/added.png")).expect("added"),
            b"added"
        );
        assert_eq!(
            std::fs::read(preset_dir.join("kept_template.png")).expect("kept"),
            b"kept",
            "a file that is not in the archive is kept"
        );
        assert_eq!(
            std::fs::read(preset_dir.join("user_note.txt")).expect("user file"),
            b"mine"
        );
        let leftovers: Vec<String> = std::fs::read_dir(root.join(PRESETS_IMAGES_SUBDIR))
            .expect("images dir")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "staging is removed: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_preset_rejects_traversal_entries_without_writing_outside_images() {
        let root = unique_tmp_root("traversal");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");
        write_preset_zip(
            &root,
            "evil.zip",
            &[
                ("9000p_EVIL/config.json", b"{}".as_slice()),
                ("9000p_EVIL/../../../evil.txt", b"escaped".as_slice()),
            ],
        );

        let message = extract_preset(&root, "evil.zip")
            .expect_err("a traversal entry must fail the extraction");
        assert!(
            message.contains("unsafe entry path"),
            "structured error must name the offending entry, got: {message}"
        );
        assert!(
            !root.join("evil.txt").exists(),
            "nothing may be written outside Images/"
        );
        let images_dir = root.join(PRESETS_IMAGES_SUBDIR);
        let leftovers: Vec<_> = std::fs::read_dir(&images_dir)
            .expect("images dir")
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(
            leftovers.is_empty(),
            "the staging tree must be cleaned up on failure: {leftovers:?}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_preset_rejects_entries_with_non_plain_path_components() {
        for (index, bad) in ["P/../Q/x.png", "/abs/x.png", "P/./x.png", "../P/x.png"]
            .iter()
            .enumerate()
        {
            let root = unique_tmp_root(&format!("components-{index}"));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create tmp root");
            write_preset_zip(
                &root,
                "P.zip",
                &[
                    ("P/config.json", b"{}".as_slice()),
                    (*bad, b"png".as_slice()),
                ],
            );

            let error = extract_preset(&root, "P.zip").expect_err(bad);

            assert!(
                error.contains("unsafe") || error.contains("absolute"),
                "{bad}: {error}"
            );
            assert!(
                !root.join(PRESETS_IMAGES_SUBDIR).join("P").exists(),
                "{bad}: nothing is extracted from a rejected archive"
            );
            assert!(!root.join(PRESETS_IMAGES_SUBDIR).join("Q").exists());
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn extract_preset_rejects_archives_without_a_single_root_directory() {
        let root = unique_tmp_root("roots");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");
        write_preset_zip(
            &root,
            "mixed.zip",
            &[
                ("preset_a/one.txt", b"a".as_slice()),
                ("preset_b/two.txt", b"b".as_slice()),
            ],
        );

        let message = extract_preset(&root, "mixed.zip")
            .expect_err("a multi-root archive must fail with a structured error");
        assert!(
            message.contains("mixes multiple root directories"),
            "got: {message}"
        );
        assert!(!root.join(PRESETS_IMAGES_SUBDIR).join("preset_a").exists());
        assert!(!root.join(PRESETS_IMAGES_SUBDIR).join("preset_b").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_preset_fails_structured_on_corrupt_archive() {
        let root = unique_tmp_root("corrupt");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");
        let images_dir = root.join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).expect("images dir");
        std::fs::write(images_dir.join("broken.zip"), b"this-is-not-a-zip")
            .expect("write corrupt zip fixture");

        let message = extract_preset(&root, "broken.zip")
            .expect_err("a corrupt archive must fail with a structured error");
        assert!(
            message.contains("not a valid zip archive"),
            "got: {message}"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_preset_failure_never_disturbs_the_previous_extraction() {
        // "Never a partial silent state": re-extracting a preset whose
        // archive is now corrupt must keep the previously extracted,
        // complete preset tree exactly as it was.
        let root = unique_tmp_root("previous");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");
        write_preset_zip(
            &root,
            "9000p_TEST.zip",
            &[
                ("9000p_TEST/config.json", preset_config_body(375).as_bytes()),
                ("9000p_TEST/tooltip_kb_all.png", b"png".as_slice()),
            ],
        );
        extract_preset(&root, "9000p_TEST.zip").expect("initial extraction");
        let preset_dir = root.join(PRESETS_IMAGES_SUBDIR).join("9000p_TEST");

        // The same preset is "re-downloaded" as garbage bytes.
        let zip_path = root.join(PRESETS_IMAGES_SUBDIR).join("9000p_TEST.zip");
        std::fs::write(&zip_path, b"garbage-not-a-zip").expect("write corrupt zip");
        assert!(extract_preset(&root, "9000p_TEST.zip").is_err());

        assert!(
            preset_dir.join("config.json").is_file(),
            "the previous extraction must survive a failed re-extraction"
        );
        assert!(
            preset_dir.join("tooltip_kb_all.png").is_file(),
            "the previous extraction must survive complete, not partial"
        );
        assert_eq!(
            std::fs::read(&zip_path).expect("zip bytes"),
            b"garbage-not-a-zip",
            "the downloaded zip stays on disk for a retry"
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_preset_rejects_a_dot_dot_root_directory_name() {
        let root = unique_tmp_root("dotdot");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("create tmp root");
        write_preset_zip(
            &root,
            "dotdot.zip",
            &[("../../escape/config.json", b"{}".as_slice())],
        );

        let message = extract_preset(&root, "dotdot.zip")
            .expect_err("a `..` root component must be rejected");
        assert!(
            message.contains("not allowed") || message.contains("unsafe"),
            "got: {message}"
        );
        assert!(!root.join("escape").exists());

        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Limits, symlinks and transactionality ─────────────────────────

    fn zip_bytes(entries: &[(&str, &[u8])], method: zip::CompressionMethod) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(method)
            .large_file(false);
        for (name, contents) in entries {
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(contents).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    /// A config root with `Images/{name}` holding `bytes`.
    fn root_with_zip(name: &str, bytes: &[u8]) -> tempfile::TempDir {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = temp.path().join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        std::fs::write(images_dir.join(name), bytes).unwrap();
        temp
    }

    fn replace_zip(temp: &tempfile::TempDir, name: &str, bytes: &[u8]) {
        std::fs::write(temp.path().join(PRESETS_IMAGES_SUBDIR).join(name), bytes).unwrap();
    }

    /// Everything under `Images/` except the archives themselves.
    fn images_tree(temp: &tempfile::TempDir) -> std::collections::BTreeMap<String, String> {
        let mut tree = snapshot(&temp.path().join(PRESETS_IMAGES_SUBDIR));
        tree.retain(|path, _| !path.ends_with(".zip"));
        tree
    }

    fn assert_no_work_dir(temp: &tempfile::TempDir) {
        assert!(!temp
            .path()
            .join(PRESETS_IMAGES_SUBDIR)
            .join(INSTALL_WORK_DIR)
            .exists());
    }

    const DEFLATE: zip::CompressionMethod = zip::CompressionMethod::Deflated;
    const STORED: zip::CompressionMethod = zip::CompressionMethod::Stored;

    fn sample_zip(config: &str) -> Vec<u8> {
        zip_bytes(
            &[
                ("1080p_X/", b""),
                ("1080p_X/config.json", config.as_bytes()),
                ("1080p_X/Tooltips/a.png", b"a"),
                ("1080p_X/Tooltips/b.png", b"b"),
            ],
            DEFLATE,
        )
    }

    /// Install the sample preset, add a user file, then return the tree.
    fn installed_sample() -> (
        tempfile::TempDir,
        std::collections::BTreeMap<String, String>,
    ) {
        let temp = root_with_zip("1080p_X.zip", &sample_zip("old"));
        super::extract_preset(temp.path(), "1080p_X.zip").unwrap();
        std::fs::write(
            temp.path()
                .join(PRESETS_IMAGES_SUBDIR)
                .join("1080p_X/user.txt"),
            b"mine",
        )
        .unwrap();
        let tree = images_tree(&temp);
        (temp, tree)
    }

    fn vendored_zip(name: &str) -> Vec<u8> {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/original/systempresets/zips")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    #[test]
    fn a_real_published_archive_extracts_completely_and_repeats_identically() {
        let bytes = vendored_zip("1080p_SMF.zip");
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes.clone())).unwrap();
        let mut expected = std::collections::BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            if entry.is_file() {
                let mut contents = Vec::new();
                entry.read_to_end(&mut contents).unwrap();
                expected.insert(entry.name().to_string(), contents);
            }
        }
        let temp = root_with_zip("1080p_SMF.zip", &bytes);

        let outcome = super::extract_preset(temp.path(), "1080p_SMF.zip").unwrap();
        assert_eq!(outcome.preset_name, "1080p_SMF");
        assert_eq!(outcome.files, expected.len());
        for (name, contents) in &expected {
            let on_disk = temp.path().join(PRESETS_IMAGES_SUBDIR).join(name);
            assert_eq!(&std::fs::read(&on_disk).unwrap(), contents, "{name}");
        }
        #[cfg(feature = "real-runtime")]
        for name in expected.keys().filter(|name| name.ends_with(".png")) {
            let image = image::open(temp.path().join(PRESETS_IMAGES_SUBDIR).join(name))
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert!(image.width() > 0 && image.height() > 0, "{name}");
        }
        let first = images_tree(&temp);
        super::extract_preset(temp.path(), "1080p_SMF.zip").unwrap();
        assert_eq!(images_tree(&temp), first, "a repeated import is idempotent");
        assert_no_work_dir(&temp);
    }

    #[test]
    fn too_many_entries_are_refused_before_anything_is_written() {
        let names: Vec<String> = (0..=MAX_ARCHIVE_ENTRIES)
            .map(|i| format!("1080p_X/{i}.png"))
            .collect();
        let entries: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b"x"[..])).collect();
        let temp = root_with_zip("1080p_X.zip", &zip_bytes(&entries, STORED));
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(
                error,
                PresetInstallError::LimitExceeded {
                    what: "entry count",
                    ..
                }
            ),
            "{error}"
        );
        assert!(images_tree(&temp).is_empty());
    }

    #[test]
    fn an_oversized_archive_file_is_refused_unread() {
        let temp = root_with_zip("1080p_X.zip", b"");
        let path = temp.path().join(PRESETS_IMAGES_SUBDIR).join("1080p_X.zip");
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(PRESET_ARCHIVE_MAX_BYTES + 1)
            .unwrap();
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(
                error,
                PresetInstallError::LimitExceeded {
                    what: "archive size (bytes)",
                    ..
                }
            ),
            "{error}"
        );
    }

    #[test]
    fn a_compression_bomb_entry_is_stopped_at_the_file_limit() {
        let zeros = vec![0_u8; MAX_EXTRACTED_FILE_BYTES as usize + 1];
        let bytes = zip_bytes(&[("1080p_X/bomb.png", &zeros)], DEFLATE);
        assert!(
            bytes.len() < 64 * 1024,
            "highly compressed: {}",
            bytes.len()
        );
        let (temp, before) = installed_sample();
        replace_zip(&temp, "1080p_X.zip", &bytes);

        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(
                error,
                PresetInstallError::LimitExceeded {
                    what: "extracted file size (bytes)",
                    ..
                }
            ),
            "{error}"
        );
        assert_eq!(images_tree(&temp), before);
        assert_no_work_dir(&temp);
    }

    /// Patch every central-directory and local-header uncompressed size to
    /// `size`, so the metadata lies about the real stream.
    fn patch_declared_sizes(bytes: &mut [u8], size: u32) {
        for index in 0..bytes.len().saturating_sub(30) {
            let field = match &bytes[index..index + 4] {
                [0x50, 0x4b, 0x01, 0x02] => index + 24,
                [0x50, 0x4b, 0x03, 0x04] => index + 22,
                _ => continue,
            };
            bytes[field..field + 4].copy_from_slice(&size.to_le_bytes());
        }
    }

    #[test]
    fn lying_entry_metadata_does_not_bypass_the_limits() {
        // Declared 100 bytes per entry; the streams hold the real data.
        let zeros = vec![0_u8; MAX_EXTRACTED_FILE_BYTES as usize + 1];
        let mut single = zip_bytes(&[("1080p_X/a.png", &zeros)], DEFLATE);
        patch_declared_sizes(&mut single, 100);
        let temp = root_with_zip("1080p_X.zip", &single);
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(
                error,
                PresetInstallError::LimitExceeded {
                    what: "extracted file size (bytes)",
                    ..
                }
            ),
            "{error}"
        );
        assert!(images_tree(&temp).is_empty());

        let chunk = vec![0_u8; 7 * 1024 * 1024];
        let names: Vec<String> = (0..10).map(|i| format!("1080p_X/{i}.png")).collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|n| (n.as_str(), chunk.as_slice()))
            .collect();
        let mut total = zip_bytes(&entries, DEFLATE);
        patch_declared_sizes(&mut total, 100);
        replace_zip(&temp, "1080p_X.zip", &total);
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(
                error,
                PresetInstallError::LimitExceeded {
                    what: "total extracted size (bytes)",
                    ..
                }
            ),
            "{error}"
        );
        assert!(images_tree(&temp).is_empty());
        assert_no_work_dir(&temp);
    }

    #[test]
    fn malformed_entry_paths_are_refused() {
        let long = format!("1080p_X/{}.png", "a".repeat(MAX_ENTRY_PATH_BYTES));
        let deep = format!("1080p_X/{}x.png", "d/".repeat(MAX_ENTRY_PATH_DEPTH));
        let cases: [(&str, &str); 6] = [
            (&long, "entry path length (bytes)"),
            (&deep, "entry path depth"),
            ("1080p_X\\..\\x.png", "unsafe"),
            ("1080p_X/a\u{1}.png", "unsafe"),
            ("1080p_X/a\u{0}.png", "unsafe"),
            (".d4lc-install/x.png", "unsafe"),
        ];
        for (name, expected) in cases {
            let temp = root_with_zip("1080p_X.zip", &zip_bytes(&[(name, b"x")], STORED));
            let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
            let matches = match &error {
                PresetInstallError::LimitExceeded { what, .. } => *what == expected,
                PresetInstallError::UnsafePath(_) => expected == "unsafe",
                _ => false,
            };
            assert!(matches, "{name:?}: {error}");
            assert!(images_tree(&temp).is_empty(), "{name:?}");
        }
    }

    #[test]
    fn archives_without_files_or_with_a_root_level_file_are_invalid() {
        for entries in [
            &[("1080p_X/", &b""[..])][..],
            &[("1080p_X/", &b""[..]), ("1080p_X/Tooltips/", &b""[..])][..],
            &[("1080p_X", &b"just a file"[..])][..],
        ] {
            let (temp, before) = installed_sample();
            replace_zip(&temp, "1080p_X.zip", &zip_bytes(entries, STORED));
            let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
            assert!(
                matches!(error, PresetInstallError::InvalidArchive(_)),
                "{entries:?}: {error}"
            );
            assert_eq!(images_tree(&temp), before, "{entries:?}");
        }
    }

    #[test]
    fn symlink_entries_are_refused() {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        writer
            .start_file(
                "1080p_X/config.json",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.write_all(b"{}").unwrap();
        writer
            .add_symlink(
                "1080p_X/link",
                "/etc/passwd",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        let bytes = writer.finish().unwrap().into_inner();
        let temp = root_with_zip("1080p_X.zip", &bytes);
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(&error, PresetInstallError::UnsafePath(m) if m.contains("symlink")),
            "{error}"
        );
        assert!(images_tree(&temp).is_empty());
    }

    #[test]
    fn truncated_and_corrupted_archives_leave_the_previous_preset_unchanged() {
        let (temp, before) = installed_sample();
        let good = sample_zip("new");
        replace_zip(&temp, "1080p_X.zip", &good[..good.len() / 2]);
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(error, PresetInstallError::InvalidArchive(_)),
            "{error}"
        );
        assert_eq!(images_tree(&temp), before);

        // A flipped byte inside the stored data fails the CRC check while
        // streaming, after the staged tree has been started.
        let mut corrupt = zip_bytes(
            &[
                ("1080p_X/config.json", b"new"),
                ("1080p_X/Tooltips/a.png", b"AAAAAAAAAAAAAAAA"),
            ],
            STORED,
        );
        let at = corrupt
            .windows(16)
            .position(|window| window == b"AAAAAAAAAAAAAAAA")
            .unwrap();
        corrupt[at] = b'B';
        replace_zip(&temp, "1080p_X.zip", &corrupt);
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(error, PresetInstallError::InvalidArchive(_)),
            "{error}"
        );
        assert_eq!(images_tree(&temp), before);
        assert_no_work_dir(&temp);
    }

    #[test]
    fn a_failure_after_the_first_or_a_later_staged_file_leaves_the_live_tree_identical() {
        for k in [1, 2, 3] {
            let (temp, before) = installed_sample();
            replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
            let hook = move |step: InstallStep| {
                if step == InstallStep::FileStaged(k) {
                    Err(std::io::Error::other("injected disk full"))
                } else {
                    Ok(())
                }
            };
            let error = extract_preset_with_hook(temp.path(), "1080p_X.zip", &hook).unwrap_err();
            assert!(
                error.to_string().contains("injected disk full"),
                "{k}: {error}"
            );
            assert_eq!(images_tree(&temp), before, "after staged file {k}");
            assert_no_work_dir(&temp);
        }
    }

    #[test]
    fn commit_step_failures_and_crashes_are_rolled_back_or_recovered() {
        for step in [InstallStep::MovePrevious, InstallStep::MoveStaged] {
            let (temp, before) = installed_sample();
            replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
            let hook = move |current: InstallStep| {
                if current == step {
                    Err(std::io::Error::other("injected rename failure"))
                } else {
                    Ok(())
                }
            };
            extract_preset_with_hook(temp.path(), "1080p_X.zip", &hook).unwrap_err();
            assert_eq!(images_tree(&temp), before, "{step:?}");
        }

        // Commit and rollback both fail: the preset is missing until the
        // recovery run (startup or next import) restores it.
        let (temp, before) = installed_sample();
        replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
        let hook = |step: InstallStep| match step {
            InstallStep::MoveStaged | InstallStep::Rollback => {
                Err(std::io::Error::other("injected rename failure"))
            }
            _ => Ok(()),
        };
        let error = extract_preset_with_hook(temp.path(), "1080p_X.zip", &hook).unwrap_err();
        assert!(
            matches!(error, PresetInstallError::RollbackFailed { .. }),
            "{error}"
        );
        crate::actions::settings::recover_interrupted_preset_installs(temp.path()).unwrap();
        assert_eq!(images_tree(&temp), before);

        // A crash between the renames, then the next import: recovery runs
        // first and the import completes on the restored tree.
        let (temp, _) = installed_sample();
        replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
        let crashed = std::panic::catch_unwind(|| {
            let hook = |step: InstallStep| {
                assert_ne!(step, InstallStep::MoveStaged, "simulated crash");
                Ok(())
            };
            extract_preset_with_hook(temp.path(), "1080p_X.zip", &hook)
        });
        assert!(crashed.is_err());
        super::extract_preset(temp.path(), "1080p_X.zip").unwrap();
        let preset = temp.path().join(PRESETS_IMAGES_SUBDIR).join("1080p_X");
        assert_eq!(std::fs::read(preset.join("config.json")).unwrap(), b"new");
        assert_eq!(std::fs::read(preset.join("user.txt")).unwrap(), b"mine");
        assert_no_work_dir(&temp);
    }

    #[test]
    fn existing_symlinks_never_redirect_writes_outside_the_preset() {
        let outside = tempfile::TempDir::new().unwrap();
        std::fs::write(outside.path().join("a.png"), b"outside").unwrap();
        let outside_before = snapshot(outside.path());

        // A symlinked subdirectory inside the live tree.
        let (temp, _) = installed_sample();
        let tooltips = temp
            .path()
            .join(PRESETS_IMAGES_SUBDIR)
            .join("1080p_X/Tooltips");
        std::fs::remove_dir_all(&tooltips).unwrap();
        std::os::unix::fs::symlink(outside.path(), &tooltips).unwrap();
        let before = images_tree(&temp);
        replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(error, PresetInstallError::SymlinkRefused(_)),
            "{error}"
        );
        assert_eq!(images_tree(&temp), before);
        assert_eq!(snapshot(outside.path()), outside_before);

        // A symlinked file at an archive path is replaced, not written through.
        let (temp, _) = installed_sample();
        let config = temp
            .path()
            .join(PRESETS_IMAGES_SUBDIR)
            .join("1080p_X/config.json");
        std::fs::remove_file(&config).unwrap();
        std::os::unix::fs::symlink(outside.path().join("a.png"), &config).unwrap();
        replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
        super::extract_preset(temp.path(), "1080p_X.zip").unwrap();
        assert!(!config.symlink_metadata().unwrap().is_symlink());
        assert_eq!(std::fs::read(&config).unwrap(), b"new");
        assert_eq!(snapshot(outside.path()), outside_before);

        // The archive itself behind a symlink.
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = temp.path().join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&images_dir).unwrap();
        let real = outside.path().join("real.zip");
        std::fs::write(&real, sample_zip("x")).unwrap();
        std::os::unix::fs::symlink(&real, images_dir.join("1080p_X.zip")).unwrap();
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(error, PresetInstallError::SymlinkRefused(_)),
            "{error}"
        );
    }

    #[test]
    fn file_directory_conflicts_and_special_files_fail_before_the_commit() {
        let (temp, _) = installed_sample();
        let config = temp
            .path()
            .join(PRESETS_IMAGES_SUBDIR)
            .join("1080p_X/config.json");
        std::fs::remove_file(&config).unwrap();
        std::fs::create_dir(&config).unwrap();
        let before = images_tree(&temp);
        replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(error, PresetInstallError::PathConflict(_)),
            "{error}"
        );
        assert_eq!(images_tree(&temp), before);

        let (temp, _) = installed_sample();
        let fifo = temp.path().join(PRESETS_IMAGES_SUBDIR).join("1080p_X/pipe");
        let status = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(status.success());
        replace_zip(&temp, "1080p_X.zip", &sample_zip("new"));
        let error = super::extract_preset(temp.path(), "1080p_X.zip").unwrap_err();
        assert!(
            matches!(error, PresetInstallError::PathConflict(_)),
            "{error}"
        );
        assert!(fifo.symlink_metadata().is_ok());
        assert_no_work_dir(&temp);
    }

    /// Live end-to-end check against the real upstream preset host through
    /// the production download and extraction functions. Run with
    /// `D4LC_LIVE_TESTS=1 cargo test -p d4lc-app --features real-runtime
    /// live_community_preset -- --ignored --nocapture`.
    #[test]
    #[cfg(all(feature = "preset-download", feature = "real-runtime"))]
    #[ignore = "live: requires network and D4LC_LIVE_TESTS=1"]
    fn live_community_preset_download_extract_and_decode() {
        use crate::actions::settings::preset_download::{
            download_preset, fetch_preset_body, DownloadError, Downloader, PRESET_DOWNLOAD_BASE_URL,
        };

        if std::env::var("D4LC_LIVE_TESTS").as_deref() != Ok("1") {
            eprintln!("[live] SKIPPED: set D4LC_LIVE_TESTS=1");
            return;
        }
        /// `UreqDownloader`'s production fetch, recording the final URL.
        struct Recording(std::sync::Mutex<Vec<(String, String)>>);
        impl Downloader for Recording {
            fn fetch(&self, url: &str, max_bytes: u64) -> Result<Vec<u8>, DownloadError> {
                let fetched = fetch_preset_body(url, max_bytes)?;
                self.0
                    .lock()
                    .unwrap()
                    .push((url.to_string(), fetched.final_url));
                Ok(fetched.bytes)
            }
        }

        let temp = tempfile::TempDir::new().unwrap();
        let downloader = Recording(std::sync::Mutex::new(Vec::new()));
        let file = "1080p_SMF.zip";
        let started = chrono::Utc::now();
        let download = download_preset(temp.path(), file, PRESET_DOWNLOAD_BASE_URL, &downloader)
            .expect("live download");
        let sha = std::process::Command::new("sha256sum")
            .arg(&download.path)
            .output()
            .unwrap();
        let (url, final_url) = downloader.0.lock().unwrap()[0].clone();
        println!("[live] fetched_at={}", started.to_rfc3339());
        println!("[live] url={url}");
        println!("[live] final_url={final_url}");
        println!("[live] bytes={}", download.bytes);
        println!(
            "[live] sha256={}",
            String::from_utf8_lossy(&sha.stdout)
                .split_whitespace()
                .next()
                .unwrap_or("?")
        );

        let extraction = super::extract_preset(temp.path(), file).expect("live extraction");
        println!(
            "[live] preset={} files={}",
            extraction.preset_name, extraction.files
        );
        let mut decoded = 0;
        for entry in walk_files(&extraction.preset_dir) {
            if entry.extension().is_some_and(|ext| ext == "png") {
                let image =
                    image::open(&entry).unwrap_or_else(|e| panic!("{}: {e}", entry.display()));
                assert!(image.width() > 0 && image.height() > 0);
                println!(
                    "[live] decoded {} {}x{}",
                    entry
                        .strip_prefix(&extraction.preset_dir)
                        .unwrap()
                        .display(),
                    image.width(),
                    image.height()
                );
                decoded += 1;
            }
        }
        assert!(
            decoded > 0,
            "the preset must contain decodable PNG templates"
        );
        println!("[live] decoded_png={decoded} RESULT=PASS");
    }

    #[cfg(all(feature = "preset-download", feature = "real-runtime"))]
    fn walk_files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk_files(&path));
            } else {
                out.push(path);
            }
        }
        out.sort();
        out
    }
}
