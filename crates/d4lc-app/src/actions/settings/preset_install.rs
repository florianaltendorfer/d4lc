//! Staged install of a preset tree into `Images/{preset}`.
//!
//! Contract: a failed install leaves the previous `Images/{preset}` tree
//! unchanged. The new tree is built completely beside the live one (a copy of
//! the live tree with the archive applied on top, so files that are not in
//! the archive are kept — upstream `ExtractToDirectory(..., overwriteFiles:
//! true)` semantics) and then swapped in with two renames inside `Images/`:
//!
//! 1. `Images/{preset}` → `Images/.d4lc-install/{preset}/old`
//! 2. `Images/.d4lc-install/{preset}/new` → `Images/{preset}`
//!
//! If step 2 fails, step 1 is undone. Between the two renames the preset
//! directory is briefly absent. The renames (and every recovery rename) run
//! under [`d4lc_assets::lock_preset_trees_exclusive`]; the runtime
//! preparation reads the preset under the shared lock, so an overlay that is
//! enabled while a download runs loads either the old or the new tree, never
//! a missing one. Staging and extraction run without the lock. A crash in
//! that window — or a failed undo — is
//! repaired by [`recover_interrupted_preset_installs`] at the next start and
//! by every later install of the same preset. The rename state is
//! self-describing: `old` present with `Images/{preset}` absent means "put
//! `old` back"; anything else in the work directory is discarded. A work
//! directory that cannot be removed (e.g. files owned by another user) is
//! moved aside to `.d4lc-install/.trash-*` instead, so it never blocks a
//! later install; startup recovery retries its removal.
//!
//! The staged copy keeps the permission bits of the live tree's files and
//! directories (a read-only directory stays read-only after the import). It
//! is owned by the current user. A file of the live tree the user cannot
//! read fails the import, leaving the live tree unchanged.
//!
//! Symlink policy (threat model: the config root belongs to the user; a
//! symlink there was planted by the same user or root, so no privilege
//! boundary exists): `Images` may be a symlink only if it resolves inside the
//! config root; below it, the install never writes through a symlink. A
//! symlink at `Images/{preset}` is refused; symlinks inside an existing
//! preset tree are kept as links, an archive file at a symlink's path
//! replaces the link, and an archive path through a symlinked directory is
//! refused.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use super::preset_download::PRESETS_IMAGES_SUBDIR;

/// Work directory under `Images/`. It contains no `p_`, so the preset lister
/// (upstream `*p_*` glob) never shows it.
pub(crate) const INSTALL_WORK_DIR: &str = ".d4lc-install";

const STAGED_DIR: &str = "new";
const PREVIOUS_DIR: &str = "old";

/// Name prefix of work directories moved aside because they could not be
/// removed. Preset names never start with `.`, so it cannot collide.
const TRASH_PREFIX: &str = ".trash-";

/// Removes an app-owned work tree; tests substitute a failing one.
type Remover<'a> = &'a dyn Fn(&Path) -> io::Result<()>;

/// Why a preset install (archive extraction included) failed.
#[derive(Debug)]
pub enum PresetInstallError {
    /// The archive is unreadable, corrupt, or not shaped like a preset
    /// (self-describing message).
    InvalidArchive(String),
    /// A resource limit was exceeded.
    LimitExceeded { what: &'static str, limit: u64 },
    /// An archive entry path or file name is unsafe or malformed
    /// (self-describing message).
    UnsafePath(String),
    /// The install would write through or replace this symlink.
    SymlinkRefused(PathBuf),
    /// `Images` resolves outside the config root.
    ImagesOutsideRoot(PathBuf),
    /// A file and a directory collide at this path, or the path holds a
    /// special file (FIFO, socket, device).
    PathConflict(PathBuf),
    /// A filesystem operation failed.
    Io {
        op: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    /// Moving the new tree into place failed and putting the previous tree
    /// back failed too. The previous tree stays in the work directory and is
    /// restored by the next recovery run.
    RollbackFailed {
        commit: Box<PresetInstallError>,
        rollback: io::Error,
    },
}

impl fmt::Display for PresetInstallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidArchive(reason) | Self::UnsafePath(reason) => f.write_str(reason),
            Self::LimitExceeded { what, limit } => {
                write!(f, "preset archive exceeds the {what} limit ({limit})")
            }
            Self::SymlinkRefused(path) => {
                write!(f, "refusing to write through symlink {}", path.display())
            }
            Self::ImagesOutsideRoot(path) => write!(
                f,
                "preset images directory {} resolves outside the config root",
                path.display()
            ),
            Self::PathConflict(path) => write!(
                f,
                "file/directory conflict or special file at {}",
                path.display()
            ),
            Self::Io { op, path, source } => {
                write!(f, "failed to {op} {}: {source}", path.display())
            }
            Self::RollbackFailed { commit, rollback } => write!(
                f,
                "{commit}; restoring the previous preset also failed ({rollback}); \
                 it is restored at the next start"
            ),
        }
    }
}

impl std::error::Error for PresetInstallError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::RollbackFailed { rollback, .. } => Some(rollback),
            _ => None,
        }
    }
}

pub(crate) fn io_error(
    op: &'static str,
    path: &Path,
) -> impl FnOnce(io::Error) -> PresetInstallError {
    let path = path.to_path_buf();
    move |source| PresetInstallError::Io { op, path, source }
}

/// Points where tests inject failures (or panics, to model a crash). The
/// production hook does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InstallStep {
    /// The n-th (1-based) archive file was written to the staged tree.
    FileStaged(usize),
    /// Before `Images/{preset}` is moved aside.
    MovePrevious,
    /// Before the staged tree is moved to `Images/{preset}`.
    MoveStaged,
    /// Before the previous tree is moved back after a failed commit.
    Rollback,
}

pub(crate) type StepHook<'a> = &'a dyn Fn(InstallStep) -> io::Result<()>;

pub(crate) fn no_hook(_: InstallStep) -> io::Result<()> {
    Ok(())
}

/// Canonical `config_root/Images`, required to resolve strictly inside the
/// canonical config root (`Images` may itself be a symlink to a directory
/// inside it). With `create`, missing directories are created first.
pub(crate) fn trusted_images_dir(
    config_root: &Path,
    create: bool,
) -> Result<PathBuf, PresetInstallError> {
    let images_dir = config_root.join(PRESETS_IMAGES_SUBDIR);
    if create {
        std::fs::create_dir_all(&images_dir)
            .map_err(io_error("create images directory", &images_dir))?;
    }
    let root = config_root
        .canonicalize()
        .map_err(io_error("resolve config root", config_root))?;
    let resolved = images_dir
        .canonicalize()
        .map_err(io_error("resolve images directory", &images_dir))?;
    if resolved == root || !resolved.starts_with(&root) {
        return Err(PresetInstallError::ImagesOutsideRoot(resolved));
    }
    if !resolved.is_dir() {
        return Err(PresetInstallError::PathConflict(resolved));
    }
    Ok(resolved)
}

/// `symlink_metadata` with "absent" as `None`.
pub(crate) fn entry_type(path: &Path) -> Result<Option<std::fs::FileType>, PresetInstallError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata.file_type())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(io_error("inspect", path)(error)),
    }
}

/// A real directory at `path`, created when absent; a symlink is refused.
fn ensure_real_dir(path: &Path) -> Result<(), PresetInstallError> {
    match entry_type(path)? {
        None => std::fs::create_dir(path).map_err(io_error("create directory", path)),
        Some(kind) if kind.is_symlink() => Err(PresetInstallError::SymlinkRefused(path.into())),
        Some(kind) if kind.is_dir() => Ok(()),
        Some(_) => Err(PresetInstallError::PathConflict(path.into())),
    }
}

/// Install a new tree for `preset_name` under the trusted `images_dir`.
///
/// `populate` receives the staged directory, which already holds a copy of
/// the current `Images/{preset_name}` tree (or is empty), applies the new
/// files and returns their count. Returns that count once the new tree is
/// live. `preset_name` must be a single plain path component that does not
/// start with `.`.
pub(crate) fn install_preset_tree(
    images_dir: &Path,
    preset_name: &str,
    populate: &mut dyn FnMut(&Path) -> Result<usize, PresetInstallError>,
    hook: StepHook<'_>,
) -> Result<usize, PresetInstallError> {
    {
        let _swap = d4lc_assets::lock_preset_trees_exclusive();
        recover_preset_install(images_dir, preset_name)?;
    }

    let target = images_dir.join(preset_name);
    let has_previous = match entry_type(&target)? {
        None => false,
        Some(kind) if kind.is_symlink() => {
            return Err(PresetInstallError::SymlinkRefused(target));
        }
        Some(kind) if kind.is_dir() => true,
        Some(_) => return Err(PresetInstallError::PathConflict(target)),
    };

    let work_root = images_dir.join(INSTALL_WORK_DIR);
    ensure_real_dir(&work_root)?;
    let work = work_root.join(preset_name);
    std::fs::create_dir(&work).map_err(io_error("create install work directory", &work))?;
    let staged = work.join(STAGED_DIR);

    let prepared = (|| {
        let mut dir_modes = Vec::new();
        if has_previous {
            copy_tree(&target, &staged, &mut dir_modes)?;
        } else {
            std::fs::create_dir(&staged).map_err(io_error("create directory", &staged))?;
        }
        let files = populate(&staged)?;
        // Directory modes are restored only now, so the archive can be
        // applied inside a read-only directory of the live tree.
        for (dir, mode) in dir_modes {
            std::fs::set_permissions(&dir, mode).map_err(io_error("set permissions of", &dir))?;
        }
        sync_tree(&staged)?;
        Ok(files)
    })();
    let files = match prepared {
        Ok(files) => files,
        Err(error) => {
            discard_work(&work_root, &work);
            return Err(error);
        }
    };

    {
        let _swap = d4lc_assets::lock_preset_trees_exclusive();
        commit(images_dir, &target, &work_root, &work, has_previous, hook)?;
    }
    discard_work(&work_root, &work);
    Ok(files)
}

/// Swap the staged tree in (module docs, steps 1–2).
fn commit(
    images_dir: &Path,
    target: &Path,
    work_root: &Path,
    work: &Path,
    has_previous: bool,
    hook: StepHook<'_>,
) -> Result<(), PresetInstallError> {
    let staged = work.join(STAGED_DIR);
    let previous = work.join(PREVIOUS_DIR);
    if has_previous {
        if let Err(error) =
            hook(InstallStep::MovePrevious).and_then(|()| std::fs::rename(target, &previous))
        {
            discard_work(work_root, work);
            return Err(io_error("move the previous preset aside", target)(error));
        }
    }
    if let Err(error) =
        hook(InstallStep::MoveStaged).and_then(|()| std::fs::rename(&staged, target))
    {
        let commit_error = io_error("move the new preset into place", target)(error);
        if has_previous {
            if let Err(rollback) =
                hook(InstallStep::Rollback).and_then(|()| std::fs::rename(&previous, target))
            {
                // The work directory is kept: recovery puts `old` back.
                return Err(PresetInstallError::RollbackFailed {
                    commit: Box::new(commit_error),
                    rollback,
                });
            }
        }
        discard_work(work_root, work);
        return Err(commit_error);
    }
    // The renames are done; a failed directory sync only weakens durability
    // across a power loss, which recovery handles like a crash.
    for dir in [images_dir, work] {
        if let Err(error) = sync_path(dir) {
            eprintln!(
                "[d4lc] preset install: cannot sync {}: {error}",
                dir.display()
            );
        }
    }
    Ok(())
}

/// [`discard_work_with`] with the production remover; a failure is only
/// logged (the next install or recovery retries).
fn discard_work(work_root: &Path, work: &Path) {
    if let Err(error) = discard_work_with(work_root, work, &remove_work_tree) {
        eprintln!("[d4lc] preset install: {error}");
    }
}

/// Remove one preset's work directory, or move it aside to
/// `{work_root}/.trash-*` when removal fails, then remove the work root if
/// it is empty. Fails only when the directory can be neither removed nor
/// moved.
fn discard_work_with(
    work_root: &Path,
    work: &Path,
    remove: Remover<'_>,
) -> Result<(), PresetInstallError> {
    if let Err(error) = remove(work) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static TRASH_COUNTER: AtomicU64 = AtomicU64::new(0);
        let name = work.file_name().unwrap_or_default().to_string_lossy();
        let trash = work_root.join(format!(
            "{TRASH_PREFIX}{name}-{}-{}",
            std::process::id(),
            TRASH_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        eprintln!(
            "[d4lc] preset install: cannot remove {}: {error}; moving it to {}",
            work.display(),
            trash.display()
        );
        std::fs::rename(work, &trash)
            .map_err(io_error("move aside the install work directory", work))?;
    }
    remove_dir_if_empty(work_root);
    Ok(())
}

/// Remove an app-owned work tree. Its directories are first made
/// owner-writable and searchable, so a read-only directory copied from the
/// live tree does not block the removal; symlinks are never followed.
fn remove_work_tree(path: &Path) -> io::Result<()> {
    make_dirs_owner_writable(path);
    match std::fs::remove_dir_all(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

/// Add `u+rwx` to `dir` and every directory below it. Failures are left for
/// the removal to report.
fn make_dirs_owner_writable(dir: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    let Ok(metadata) = std::fs::symlink_metadata(dir) else {
        return;
    };
    if !metadata.is_dir() {
        return;
    }
    let mode = metadata.permissions().mode();
    if mode & 0o700 != 0o700 {
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(mode | 0o700));
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            make_dirs_owner_writable(&entry.path());
        }
    }
}

fn remove_dir_if_empty(dir: &Path) {
    if let Err(error) = std::fs::remove_dir(dir) {
        if !matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::DirectoryNotEmpty
        ) {
            eprintln!(
                "[d4lc] preset install: cannot remove {}: {error}",
                dir.display()
            );
        }
    }
}

/// Copy `source` to the new directory `destination`: directories recreated
/// (writable; their source modes are appended to `dir_modes`, children
/// before parents, for the caller to apply), regular files copied with
/// their permission bits, symlinks recreated as the same links (never
/// followed). Special files fail with [`PresetInstallError::PathConflict`].
fn copy_tree(
    source: &Path,
    destination: &Path,
    dir_modes: &mut Vec<(PathBuf, std::fs::Permissions)>,
) -> Result<(), PresetInstallError> {
    let mode = std::fs::symlink_metadata(source)
        .map_err(io_error("inspect", source))?
        .permissions();
    std::fs::create_dir(destination).map_err(io_error("create directory", destination))?;
    let entries = std::fs::read_dir(source).map_err(io_error("read directory", source))?;
    for entry in entries {
        let entry = entry.map_err(io_error("read directory", source))?;
        let from = entry.path();
        let to = destination.join(entry.file_name());
        let kind = entry.file_type().map_err(io_error("inspect", &from))?;
        if kind.is_dir() {
            copy_tree(&from, &to, dir_modes)?;
        } else if kind.is_file() {
            std::fs::copy(&from, &to).map_err(io_error("copy", &from))?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(&from).map_err(io_error("read symlink", &from))?;
            std::os::unix::fs::symlink(&link, &to).map_err(io_error("create symlink", &to))?;
        } else {
            return Err(PresetInstallError::PathConflict(from));
        }
    }
    dir_modes.push((destination.to_path_buf(), mode));
    Ok(())
}

/// `fsync` every regular file and directory of the tree (symlinks skipped),
/// so a committed tree survives a power loss with its content.
fn sync_tree(dir: &Path) -> Result<(), PresetInstallError> {
    let entries = std::fs::read_dir(dir).map_err(io_error("read directory", dir))?;
    for entry in entries {
        let entry = entry.map_err(io_error("read directory", dir))?;
        let path = entry.path();
        let kind = entry.file_type().map_err(io_error("inspect", &path))?;
        if kind.is_dir() {
            sync_tree(&path)?;
        } else if kind.is_file() {
            sync_path(&path).map_err(io_error("sync", &path))?;
        }
    }
    sync_path(dir).map_err(io_error("sync", dir))
}

fn sync_path(path: &Path) -> io::Result<()> {
    std::fs::File::open(path)?.sync_all()
}

/// Finish or undo an interrupted install of `preset_name` (module docs) and
/// discard its work directory.
pub(crate) fn recover_preset_install(
    images_dir: &Path,
    preset_name: &str,
) -> Result<(), PresetInstallError> {
    recover_preset_install_with(images_dir, preset_name, &remove_work_tree)
}

fn recover_preset_install_with(
    images_dir: &Path,
    preset_name: &str,
    remove: Remover<'_>,
) -> Result<(), PresetInstallError> {
    let work_root = images_dir.join(INSTALL_WORK_DIR);
    match entry_type(&work_root)? {
        None => return Ok(()),
        Some(kind) if kind.is_symlink() => {
            return Err(PresetInstallError::SymlinkRefused(work_root));
        }
        Some(kind) if !kind.is_dir() => return Err(PresetInstallError::PathConflict(work_root)),
        Some(_) => {}
    }
    let work = work_root.join(preset_name);
    match entry_type(&work)? {
        None => return Ok(()),
        Some(kind) if !kind.is_dir() => {
            // A stray file or symlink in the app's own work root: remove the
            // entry itself (a symlink is not followed).
            std::fs::remove_file(&work).map_err(io_error("remove stray work entry", &work))?;
            remove_dir_if_empty(&work_root);
            return Ok(());
        }
        Some(_) => {}
    }
    let previous = work.join(PREVIOUS_DIR);
    let target = images_dir.join(preset_name);
    if entry_type(&previous)?.is_some_and(|kind| kind.is_dir()) && entry_type(&target)?.is_none() {
        std::fs::rename(&previous, &target)
            .map_err(io_error("restore the previous preset", &target))?;
        eprintln!(
            "[d4lc] preset install: restored {} after an interrupted install",
            target.display()
        );
    }
    discard_work_with(&work_root, &work, remove)
}

/// Repair every interrupted preset install under `config_root/Images` (see
/// the module docs), retry removing moved-aside work directories, and remove
/// staging directories left by older versions (`Images/.{preset}.extracting`,
/// which the preset lister would show). Does nothing when `Images` does not
/// exist. Run once at startup.
///
/// Each entry is handled independently: a failure is logged and the
/// remaining entries are still processed; the first failure is returned.
/// Failing to remove a `.trash-*` directory is only logged.
pub fn recover_interrupted_preset_installs(config_root: &Path) -> Result<(), PresetInstallError> {
    if entry_type(&config_root.join(PRESETS_IMAGES_SUBDIR))?.is_none() {
        return Ok(());
    }
    let images_dir = trusted_images_dir(config_root, false)?;
    let mut first_error = None;
    let mut record = |result: Result<(), PresetInstallError>| {
        if let Err(error) = result {
            eprintln!("[d4lc] preset install recovery: {error}");
            first_error.get_or_insert(error);
        }
    };

    let work_root = images_dir.join(INSTALL_WORK_DIR);
    if entry_type(&work_root)?.is_some_and(|kind| kind.is_dir()) {
        let entries =
            std::fs::read_dir(&work_root).map_err(io_error("read directory", &work_root))?;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    record(Err(io_error("read directory", &work_root)(error)));
                    continue;
                }
            };
            let path = entry.path();
            let name = entry.file_name();
            match name.to_str() {
                Some(name) if name.starts_with(TRASH_PREFIX) => {
                    if let Err(error) = remove_work_tree(&path) {
                        eprintln!(
                            "[d4lc] preset install recovery: cannot remove {}: {error}",
                            path.display()
                        );
                    }
                }
                Some(name) => record({
                    let _swap = d4lc_assets::lock_preset_trees_exclusive();
                    recover_preset_install(&images_dir, name)
                }),
                None => record(Err(PresetInstallError::UnsafePath(format!(
                    "non-UTF-8 name {}",
                    path.display()
                )))),
            }
        }
        remove_dir_if_empty(&work_root);
    }

    let entries =
        std::fs::read_dir(&images_dir).map_err(io_error("read directory", &images_dir))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_dir = entry.file_type().is_ok_and(|kind| kind.is_dir());
        if is_dir && name.starts_with('.') && name.ends_with(".extracting") {
            let path = entry.path();
            record(remove_work_tree(&path).map_err(io_error("remove stale staging", &path)));
        }
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// Every entry below `dir` (relative path → file bytes, `dir`, or
    /// `link -> target`), for byte-identical tree comparisons.
    pub(crate) fn snapshot(dir: &Path) -> BTreeMap<String, String> {
        fn walk(base: &Path, dir: &Path, out: &mut BTreeMap<String, String>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let key = path.strip_prefix(base).unwrap().display().to_string();
                let kind = entry.file_type().unwrap();
                if kind.is_symlink() {
                    let link = std::fs::read_link(&path).unwrap();
                    out.insert(key, format!("link -> {}", link.display()));
                } else if kind.is_dir() {
                    out.insert(key, "dir".to_string());
                    walk(base, &path, out);
                } else {
                    let bytes = std::fs::read(&path).unwrap();
                    out.insert(key, format!("file {bytes:?}"));
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(dir, dir, &mut out);
        out
    }

    fn images(temp: &tempfile::TempDir) -> PathBuf {
        let dir = temp.path().join(PRESETS_IMAGES_SUBDIR);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn write_files(dir: &Path, files: &[(&str, &str)]) {
        for (name, body) in files {
            let path = dir.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
    }

    fn populate_with<'a>(
        files: &'a [(&'a str, &'a str)],
    ) -> impl FnMut(&Path) -> Result<usize, PresetInstallError> + 'a {
        move |staged: &Path| {
            for (name, body) in files {
                let path = staged.join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                let _ = std::fs::remove_file(&path);
                std::fs::write(&path, body).unwrap();
            }
            Ok(files.len())
        }
    }

    fn failing_at(step: InstallStep) -> impl Fn(InstallStep) -> io::Result<()> {
        move |current| {
            if current == step {
                Err(io::Error::other(format!("injected failure at {current:?}")))
            } else {
                Ok(())
            }
        }
    }

    /// Copy the vendored `1080p_SMF` preset (a complete, loadable preset)
    /// to `images/{name}`.
    fn vendored_preset_copy(images: &Path, name: &str) {
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/original/systempresets/1080p_SMF");
        fn copy_dir(from: &Path, to: &Path) {
            std::fs::create_dir_all(to).unwrap();
            for entry in std::fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let target = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy_dir(&entry.path(), &target);
                } else {
                    std::fs::copy(entry.path(), target).unwrap();
                }
            }
        }
        copy_dir(&source, &images.join(name));
    }

    /// A preset read that coincides with the swap of the same preset waits
    /// for the swap and then loads the complete new tree; it never sees the
    /// preset missing (the window between the two renames is real, as the
    /// first assertion shows).
    #[test]
    fn a_preset_read_during_the_swap_sees_the_new_tree_never_a_missing_one() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let config_root = temp.path().canonicalize().unwrap();
        vendored_preset_copy(&images_dir, "1080p_Swap");
        std::fs::write(images_dir.join("1080p_Swap/version.txt"), "old").unwrap();

        let (between_tx, between_rx) = std::sync::mpsc::channel();
        let installer = {
            let images_dir = images_dir.clone();
            std::thread::spawn(move || {
                let hook = move |step: InstallStep| {
                    if step == InstallStep::MoveStaged {
                        between_tx.send(()).unwrap();
                        // Keep the window open long enough for the reader to
                        // arrive while the preset directory is absent.
                        std::thread::sleep(std::time::Duration::from_millis(300));
                    }
                    Ok(())
                };
                install_preset_tree(
                    &images_dir,
                    "1080p_Swap",
                    &mut populate_with(&[("version.txt", "new")]),
                    &hook,
                )
            })
        };

        between_rx.recv().unwrap();
        assert!(
            !images_dir.join("1080p_Swap").exists(),
            "the preset is absent between the renames"
        );
        let started = std::time::Instant::now();
        let (loaded, version) = {
            let _presets = d4lc_assets::lock_preset_trees_shared();
            let loaded =
                d4lc_assets::AssetRepository::new(&config_root).load_preset_templates("1080p_Swap");
            let version = std::fs::read_to_string(images_dir.join("1080p_Swap/version.txt"));
            (loaded, version)
        };
        let waited = started.elapsed();

        let loaded = loaded.unwrap_or_else(|error| panic!("read during the swap failed: {error}"));
        assert!(!loaded.templates.is_empty());
        assert_eq!(
            version.unwrap(),
            "new",
            "the read sees the complete new tree"
        );
        assert!(
            waited >= std::time::Duration::from_millis(100),
            "the read waited for the swap ({waited:?})"
        );
        assert_eq!(installer.join().unwrap().unwrap(), 1);
    }

    #[test]
    fn install_creates_a_new_preset_and_cleans_its_work_directory() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let files = install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("config.json", "{}"), ("Tooltips/a.png", "a")]),
            &no_hook,
        )
        .unwrap();
        assert_eq!(files, 2);
        assert_eq!(
            std::fs::read_to_string(images_dir.join("1080p_X/Tooltips/a.png")).unwrap(),
            "a"
        );
        assert!(!images_dir.join(INSTALL_WORK_DIR).exists());
    }

    #[test]
    fn install_keeps_files_not_in_the_new_set_and_preserves_symlinks_as_links() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let target = images_dir.join("1080p_X");
        write_files(&target, &[("config.json", "old"), ("user.txt", "mine")]);
        std::os::unix::fs::symlink("/nonexistent/elsewhere", target.join("link")).unwrap();

        install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("config.json", "new")]),
            &no_hook,
        )
        .unwrap();

        let tree = snapshot(&target);
        assert_eq!(tree["config.json"], format!("file {:?}", b"new"));
        assert_eq!(tree["user.txt"], format!("file {:?}", b"mine"));
        assert_eq!(tree["link"], "link -> /nonexistent/elsewhere");
    }

    #[test]
    fn a_failure_at_any_commit_step_leaves_the_live_tree_byte_identical() {
        for step in [InstallStep::MovePrevious, InstallStep::MoveStaged] {
            let temp = tempfile::TempDir::new().unwrap();
            let images_dir = images(&temp);
            let target = images_dir.join("1080p_X");
            write_files(&target, &[("config.json", "old"), ("a/b.png", "b")]);
            let before = snapshot(&target);

            let error = install_preset_tree(
                &images_dir,
                "1080p_X",
                &mut populate_with(&[("config.json", "new"), ("a/c.png", "c")]),
                &failing_at(step),
            )
            .expect_err("injected failure");

            assert!(
                error.to_string().contains("injected failure"),
                "{step:?}: {error}"
            );
            assert_eq!(snapshot(&target), before, "{step:?}");
            assert!(!images_dir.join(INSTALL_WORK_DIR).exists(), "{step:?}");
        }
    }

    #[test]
    fn a_failed_rollback_is_reported_and_repaired_by_recovery() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let target = images_dir.join("1080p_X");
        write_files(&target, &[("config.json", "old"), ("a/b.png", "b")]);
        let before = snapshot(&target);

        let hook = |step: InstallStep| match step {
            InstallStep::MoveStaged | InstallStep::Rollback => {
                Err(io::Error::other(format!("injected failure at {step:?}")))
            }
            _ => Ok(()),
        };
        let error = install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("config.json", "new")]),
            &hook,
        )
        .expect_err("commit and rollback fail");
        assert!(
            matches!(error, PresetInstallError::RollbackFailed { .. }),
            "{error}"
        );
        assert!(
            !target.exists(),
            "the window state the recovery has to repair"
        );

        recover_interrupted_preset_installs(temp.path()).unwrap();
        assert_eq!(snapshot(&target), before);
        assert!(!images_dir.join(INSTALL_WORK_DIR).exists());
    }

    #[test]
    fn a_crash_between_the_renames_is_rolled_back_by_recovery() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let target = images_dir.join("1080p_X");
        write_files(&target, &[("config.json", "old")]);
        let before = snapshot(&target);

        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let hook = |step: InstallStep| {
                assert_ne!(step, InstallStep::MoveStaged, "simulated crash");
                Ok(())
            };
            install_preset_tree(
                &images_dir,
                "1080p_X",
                &mut populate_with(&[("config.json", "new")]),
                &hook,
            )
        }));
        assert!(crashed.is_err());
        assert!(!target.exists());

        recover_interrupted_preset_installs(temp.path()).unwrap();
        assert_eq!(snapshot(&target), before);
        assert!(!images_dir.join(INSTALL_WORK_DIR).exists());
    }

    #[test]
    fn a_crash_after_the_commit_keeps_the_new_tree_and_the_next_install_cleans_up() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let target = images_dir.join("1080p_X");
        write_files(&target, &[("config.json", "old")]);
        // State after rename 2, before the work directory was removed.
        let work = images_dir.join(INSTALL_WORK_DIR).join("1080p_X");
        std::fs::create_dir_all(work.join(PREVIOUS_DIR)).unwrap();
        std::fs::write(work.join(PREVIOUS_DIR).join("config.json"), "older").unwrap();

        install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("added.png", "x")]),
            &no_hook,
        )
        .unwrap();
        let tree = snapshot(&target);
        assert_eq!(tree["config.json"], format!("file {:?}", b"old"));
        assert_eq!(tree["added.png"], format!("file {:?}", b"x"));
        assert!(!images_dir.join(INSTALL_WORK_DIR).exists());
    }

    #[test]
    fn a_crash_before_the_commit_discards_the_staged_tree() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let work = images_dir.join(INSTALL_WORK_DIR).join("1080p_X");
        write_files(&work.join(STAGED_DIR), &[("config.json", "half")]);

        recover_interrupted_preset_installs(temp.path()).unwrap();
        assert!(!images_dir.join("1080p_X").exists());
        assert!(!images_dir.join(INSTALL_WORK_DIR).exists());
    }

    #[test]
    fn recovery_removes_legacy_staging_directories() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        write_files(
            &images_dir.join(".1080p_X.extracting"),
            &[("config.json", "x")],
        );
        write_files(&images_dir.join("1080p_X"), &[("config.json", "keep")]);
        recover_interrupted_preset_installs(temp.path()).unwrap();
        assert!(!images_dir.join(".1080p_X.extracting").exists());
        assert!(images_dir.join("1080p_X/config.json").is_file());
    }

    #[test]
    fn recovery_resolves_every_reachable_rename_state() {
        // (old, new, live present) -> live content after recovery.
        let cases: [(bool, bool, bool, Option<&str>); 7] = [
            (true, true, false, Some("old")),
            (true, false, false, Some("old")),
            (true, true, true, Some("live")),
            (true, false, true, Some("live")),
            (false, true, true, Some("live")),
            (false, true, false, None),
            (false, false, false, None),
        ];
        for (old, new, live, expected) in cases {
            let temp = tempfile::TempDir::new().unwrap();
            let images_dir = images(&temp);
            let work = images_dir.join(INSTALL_WORK_DIR).join("1080p_X");
            std::fs::create_dir_all(&work).unwrap();
            if old {
                write_files(&work.join(PREVIOUS_DIR), &[("config.json", "old")]);
            }
            if new {
                write_files(&work.join(STAGED_DIR), &[("config.json", "new")]);
            }
            if live {
                write_files(&images_dir.join("1080p_X"), &[("config.json", "live")]);
            }
            recover_interrupted_preset_installs(temp.path()).unwrap();
            let got = std::fs::read_to_string(images_dir.join("1080p_X/config.json")).ok();
            assert_eq!(got.as_deref(), expected, "old={old} new={new} live={live}");
            assert!(!images_dir.join(INSTALL_WORK_DIR).exists());
        }
    }

    /// Give every directory below `dir` back `u+w` so `TempDir` can drop.
    fn unlock(dir: &Path) {
        make_dirs_owner_writable(dir);
    }

    fn mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::symlink_metadata(path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn a_read_only_directory_in_the_live_tree_never_blocks_later_installs() {
        use std::os::unix::fs::PermissionsExt as _;
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let target = images_dir.join("1080p_X");
        write_files(&target, &[("sub/a.png", "a")]);
        std::fs::set_permissions(target.join("sub"), std::fs::Permissions::from_mode(0o555))
            .unwrap();

        for (round, body) in [(1, "one"), (2, "two")] {
            let files = install_preset_tree(
                &images_dir,
                "1080p_X",
                &mut populate_with(&[("config.json", body), ("sub/b.png", body)]),
                &no_hook,
            )
            .unwrap_or_else(|error| panic!("install {round}: {error}"));
            assert_eq!(files, 2);
            assert_eq!(
                std::fs::read_to_string(target.join("config.json")).unwrap(),
                body
            );
            assert_eq!(
                std::fs::read_to_string(target.join("sub/b.png")).unwrap(),
                body
            );
            assert_eq!(mode(&target.join("sub")), 0o555, "round {round}: mode kept");
            assert!(!images_dir.join(INSTALL_WORK_DIR).exists(), "round {round}");
        }
        recover_interrupted_preset_installs(temp.path()).unwrap();
        unlock(temp.path());
    }

    #[test]
    fn an_unremovable_work_directory_is_moved_aside_and_retried_at_startup() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        write_files(&images_dir.join("1080p_X"), &[("config.json", "live")]);
        let work_root = images_dir.join(INSTALL_WORK_DIR);
        write_files(
            &work_root.join("1080p_X").join(PREVIOUS_DIR),
            &[("config.json", "old")],
        );

        // Removal fails (e.g. files owned by root): the leftover is moved
        // aside instead of failing every later install.
        let refuse = |_: &Path| Err(io::Error::from(io::ErrorKind::PermissionDenied));
        recover_preset_install_with(&images_dir, "1080p_X", &refuse).unwrap();
        assert!(!work_root.join("1080p_X").exists());
        let trash: Vec<String> = std::fs::read_dir(&work_root)
            .unwrap()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(trash.len(), 1);
        assert!(trash[0].starts_with(".trash-1080p_X-"), "{trash:?}");

        install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("config.json", "new")]),
            &no_hook,
        )
        .expect("the leftover does not block the install");
        assert_eq!(
            std::fs::read_to_string(images_dir.join("1080p_X/config.json")).unwrap(),
            "new"
        );
        recover_interrupted_preset_installs(temp.path()).unwrap();
        assert!(!work_root.exists(), "startup removes the moved-aside tree");
    }

    #[test]
    fn startup_recovery_handles_every_entry_despite_failures() {
        use std::os::unix::ffi::OsStrExt as _;
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let work_root = images_dir.join(INSTALL_WORK_DIR);
        std::fs::create_dir_all(&work_root).unwrap();
        std::fs::write(work_root.join("aaa_stray"), "x").unwrap();
        std::fs::create_dir(work_root.join(std::ffi::OsStr::from_bytes(b"bad\xff"))).unwrap();
        write_files(
            &work_root.join("zzz_1080p_X").join(PREVIOUS_DIR),
            &[("config.json", "old")],
        );
        write_files(
            &images_dir.join(".1080p_Y.extracting"),
            &[("config.json", "x")],
        );

        let error = recover_interrupted_preset_installs(temp.path())
            .expect_err("the non-UTF-8 entry is reported");
        assert!(
            matches!(error, PresetInstallError::UnsafePath(_)),
            "{error}"
        );
        assert!(!work_root.join("aaa_stray").exists(), "stray entry removed");
        assert_eq!(
            std::fs::read_to_string(images_dir.join("zzz_1080p_X/config.json")).unwrap(),
            "old",
            "entries after a failing one are still recovered"
        );
        assert!(!images_dir.join(".1080p_Y.extracting").exists());
    }

    #[test]
    fn recovery_without_an_images_directory_does_nothing() {
        let temp = tempfile::TempDir::new().unwrap();
        recover_interrupted_preset_installs(temp.path()).unwrap();
        assert!(!temp.path().join(PRESETS_IMAGES_SUBDIR).exists());
    }

    #[test]
    fn a_symlinked_preset_directory_is_refused_and_its_target_untouched() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let outside = temp.path().join("outside");
        write_files(&outside, &[("config.json", "outside")]);
        std::os::unix::fs::symlink(&outside, images_dir.join("1080p_X")).unwrap();
        let before = snapshot(&outside);

        let error = install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("config.json", "new")]),
            &no_hook,
        )
        .expect_err("symlinked preset dir");
        assert!(
            matches!(error, PresetInstallError::SymlinkRefused(_)),
            "{error}"
        );
        assert_eq!(snapshot(&outside), before);
    }

    #[test]
    fn a_symlinked_work_directory_is_refused() {
        let temp = tempfile::TempDir::new().unwrap();
        let images_dir = images(&temp);
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, images_dir.join(INSTALL_WORK_DIR)).unwrap();

        let error = install_preset_tree(
            &images_dir,
            "1080p_X",
            &mut populate_with(&[("config.json", "new")]),
            &no_hook,
        )
        .expect_err("symlinked work dir");
        assert!(
            matches!(error, PresetInstallError::SymlinkRefused(_)),
            "{error}"
        );
        assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
        assert!(!images_dir.join("1080p_X").exists());
    }

    #[test]
    fn images_may_be_a_symlink_inside_the_config_root_only() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("config");
        std::fs::create_dir_all(root.join("real-images")).unwrap();
        std::os::unix::fs::symlink("real-images", root.join(PRESETS_IMAGES_SUBDIR)).unwrap();
        assert_eq!(
            trusted_images_dir(&root, false).unwrap(),
            root.join("real-images").canonicalize().unwrap()
        );

        let other = temp.path().join("other");
        std::fs::create_dir_all(other.join(PRESETS_IMAGES_SUBDIR)).unwrap();
        std::fs::remove_file(root.join(PRESETS_IMAGES_SUBDIR)).unwrap();
        std::os::unix::fs::symlink(
            other.join(PRESETS_IMAGES_SUBDIR),
            root.join(PRESETS_IMAGES_SUBDIR),
        )
        .unwrap();
        assert!(matches!(
            trusted_images_dir(&root, false),
            Err(PresetInstallError::ImagesOutsideRoot(_))
        ));
        std::fs::remove_file(root.join(PRESETS_IMAGES_SUBDIR)).unwrap();
        std::os::unix::fs::symlink(".", root.join(PRESETS_IMAGES_SUBDIR)).unwrap();
        assert!(matches!(
            trusted_images_dir(&root, false),
            Err(PresetInstallError::ImagesOutsideRoot(_))
        ));
    }
}
