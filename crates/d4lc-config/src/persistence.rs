//! Atomic, revision-stamped persistence for the configuration files.
//!
//! Every write hands the new bytes to a temp file in the target's own
//! directory, syncs it, and only then renames it over the target. A
//! reader therefore never observes a half-written configuration file,
//! and a failed write leaves the previous bytes (and the previously
//! applied revision) exactly as they were.
//!
//! A change that spans more than one file uses a small transaction
//! record under `Config/.persistence`: the record is written before the
//! first replacement and removed after the last one, so an interrupted
//! change is visible at the next start instead of being silently
//! adopted. Recovery is explicit ([`InterruptedTransaction::recover`],
//! [`Recovery`]); nothing here completes or discards a partial state on
//! its own.
//!
//! Durability order of a transaction: staged files and backups are
//! synced (contents and directory entries) before the record is
//! atomically published; every replaced target's directory is synced
//! before the record is removed; the backups go last. Single-file writes,
//! transactions and recoveries never overlap, also across processes (the
//! app and the diagnostics CLI): each holds an advisory `flock` on
//! [`JOURNAL_DIR`]`/lock` from its unresolved-record check through its last
//! rename. The kernel drops that lock when its holder dies, so a killed
//! process never leaves a lock that blocks later saves.
//!
//! The returned [`CommittedRevision`] is derived from the bytes that
//! were actually persisted, so a caller can only report a new applied
//! revision after a write that really reached the disk.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

/// Directory, relative to the config root, that holds transaction
/// records and the previous-content backups of an in-flight change.
pub const JOURNAL_DIR: &str = "Config/.persistence";

const JOURNAL_PREFIX: &str = "txn-";
const JOURNAL_SUFFIX: &str = ".json";
const JOURNAL_LOCK: &str = "lock";
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// One file of a multi-file change.
#[derive(Debug, Clone, Copy)]
pub struct FileWrite<'a> {
    /// Path relative to the config root (for example `Config/Settings.json`).
    pub relative_path: &'a str,
    /// The bytes to persist.
    pub contents: &'a str,
}

/// A configuration change that reached the disk.
///
/// The digest covers the paths and bytes of every committed file. It is a
/// non-cryptographic change marker, not proof that two revisions are identical.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommittedRevision {
    /// FNV-1a digest over every committed file (relative path + bytes).
    pub digest: u64,
    /// The config-root-relative paths replaced by the commit, in
    /// replacement order.
    pub files: Vec<String>,
}

/// Why a configuration change could not be persisted.
#[derive(Debug)]
pub enum ConfigError {
    /// A filesystem operation failed.
    Io {
        path: PathBuf,
        action: &'static str,
        source: std::io::Error,
    },
    /// A configuration document could not be parsed as its JSON shape.
    Malformed { path: PathBuf, detail: String },
    /// The file was written by a newer schema version this build does
    /// not understand; overwriting it would silently drop its unknown
    /// fields.
    NewerSchema {
        path: String,
        version: u32,
        supported_version: u32,
    },
    /// An earlier change left an unresolved transaction record; writing
    /// over its files would silently adopt a partial state.
    PendingRecovery { record: PathBuf, detail: String },
    /// The change failed between file replacements; the record under
    /// `record` describes the partial state and requires explicit
    /// recovery.
    CommitInterrupted { record: PathBuf, detail: String },
    /// An explicit recovery could not be completed safely.
    RecoveryFailed { record: PathBuf, detail: String },
}

impl ConfigError {
    /// Build a `Malformed` error for one configuration document.
    pub fn malformed(path: impl Into<PathBuf>, detail: impl Into<String>) -> Self {
        Self::Malformed {
            path: path.into(),
            detail: detail.into(),
        }
    }

    /// Build an `Io` error for a failed filesystem operation.
    pub(crate) fn io(
        path: impl Into<PathBuf>,
        action: &'static str,
    ) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io {
            path,
            action,
            source,
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io {
                path,
                action,
                source,
            } => write!(formatter, "failed to {action} {}: {source}", path.display()),
            Self::Malformed { path, detail } => write!(
                formatter,
                "{} is not a valid configuration document: {detail}",
                path.display()
            ),
            Self::NewerSchema {
                path,
                version,
                supported_version,
            } => write!(
                formatter,
                "{path} schema_version {version} is newer than supported version {supported_version}; \
                 refusing to overwrite (upgrade the application or remove the file to save)"
            ),
            Self::PendingRecovery { record, detail } => write!(
                formatter,
                "an earlier configuration change is unresolved ({}): {detail}",
                record.display()
            ),
            Self::CommitInterrupted { record, detail } => write!(
                formatter,
                "configuration change interrupted: {detail}; recovery record {}",
                record.display()
            ),
            Self::RecoveryFailed { record, detail } => write!(
                formatter,
                "configuration recovery for {} failed: {detail}",
                record.display()
            ),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}

impl From<ConfigError> for String {
    fn from(error: ConfigError) -> Self {
        error.to_string()
    }
}

/// Persist one file atomically and report the committed revision.
///
/// # Errors
///
/// Fails before touching the target when it is part of an unresolved
/// change (the caller must recover that change explicitly first), and
/// reports any staging or replacement failure without altering the
/// target.
pub fn write_atomic(
    root: &Path,
    relative_path: &str,
    contents: &str,
) -> Result<CommittedRevision, ConfigError> {
    validate_target_path(relative_path)?;
    // Held through the rename: a transaction or recovery that starts after
    // the check below cannot resolve this file underneath the write.
    let _lock = lock_journal(root)?;
    if let Some(record) = unresolved_record_for(root, relative_path)? {
        return Err(ConfigError::PendingRecovery {
            record,
            detail: format!(
                "{relative_path} belongs to an interrupted change; complete or roll back that change first"
            ),
        });
    }
    let target = root.join(relative_path);
    let (temp, _) = stage_paths(&target, relative_path, &transaction_id(), 0)?;
    write_new_file(&temp, contents.as_bytes())?;
    #[cfg(test)]
    pause_point();
    if let Err(error) = replace_with_temp(&temp, &target) {
        let _ = fs::remove_file(&temp);
        return Err(error);
    }
    Ok(CommittedRevision {
        digest: digest_of_files([(relative_path.to_string(), contents.as_bytes())]),
        files: vec![relative_path.to_string()],
    })
}

/// Prepare a multi-file change before any target is replaced.
///
/// Every file is staged and synced first, and a transaction record is
/// written so the change is visible as in-flight. The caller then calls
/// [`PreparedTransaction::commit`]; a prepare failure removes everything
/// it created and changes no target.
///
/// # Errors
///
/// Refuses to start while any earlier transaction record is unresolved.
pub fn prepare_transaction(
    root: &Path,
    writes: &[FileWrite<'_>],
) -> Result<PreparedTransaction, ConfigError> {
    if writes.is_empty() {
        return Err(ConfigError::malformed(
            root,
            "a configuration transaction needs at least one file",
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for write in writes {
        validate_target_path(write.relative_path)?;
        if !paths.insert(write.relative_path) {
            return Err(ConfigError::malformed(root, "duplicate transaction target"));
        }
    }
    let lock = lock_journal(root)?;
    let journal = root.join(JOURNAL_DIR);
    if let Some(record) = first_record(&journal)? {
        return Err(ConfigError::PendingRecovery {
            record,
            detail: "another configuration change is unresolved; complete or roll it back first"
                .to_string(),
        });
    }

    let id = transaction_id();
    let mut files = Vec::new();
    for (index, write) in writes.iter().enumerate() {
        match stage_file(root, &id, index, write) {
            Ok(file) => files.push(file),
            Err(error) => {
                discard_prepared(root, &files);
                return Err(error);
            }
        }
    }

    // The record may only name files whose bytes and directory entries
    // are durable.
    if let Err(error) = staging_directories(root, &files)
        .iter()
        .try_for_each(|directory| sync_directory(directory))
    {
        discard_prepared(root, &files);
        return Err(error);
    }
    #[cfg(test)]
    crash_point(CrashAfter::Staging);

    // Published by rename, so a crash never leaves a torn record (which
    // would block every later write without naming its files).
    let record_path = journal.join(format!("{JOURNAL_PREFIX}{id}{JOURNAL_SUFFIX}"));
    let record_temp = journal.join(format!(".{JOURNAL_PREFIX}{id}{JOURNAL_SUFFIX}.tmp"));
    let record = record_json(&files);
    if let Err(error) = write_new_file(&record_temp, record.as_bytes())
        .and_then(|()| {
            fs::rename(&record_temp, &record_path).map_err(ConfigError::io(&record_path, "publish"))
        })
        .and_then(|()| sync_directory(&journal))
    {
        let _ = fs::remove_file(&record_temp);
        let _ = fs::remove_file(&record_path);
        discard_prepared(root, &files);
        return Err(error);
    }
    #[cfg(test)]
    crash_point(CrashAfter::Record);
    let digest = digest_of_files(
        writes
            .iter()
            .map(|write| (write.relative_path.to_string(), write.contents.as_bytes())),
    );
    Ok(PreparedTransaction {
        root: root.to_path_buf(),
        record_path,
        files,
        digest,
        _lock: lock,
    })
}

/// A staged multi-file change that has not been applied yet.
#[derive(Debug)]
pub struct PreparedTransaction {
    root: PathBuf,
    record_path: PathBuf,
    files: Vec<PreparedFile>,
    digest: u64,
    /// The journal lock, held until the transaction is committed or
    /// dropped (a dropped, uncommitted transaction stays interrupted).
    _lock: fs::File,
}

impl PreparedTransaction {
    /// The transaction record that marks this change as in-flight.
    pub fn record_path(&self) -> &Path {
        &self.record_path
    }

    /// Replace every target with its prepared bytes and finish the
    /// transaction.
    ///
    /// # Errors
    ///
    /// A failure between replacements returns
    /// [`ConfigError::CommitInterrupted`] and leaves the record in
    /// place: the files replaced so far carry the new revision, the
    /// rest keep the previous one, and recovery is explicit.
    pub fn commit(self) -> Result<CommittedRevision, ConfigError> {
        for (replaced, file) in self.files.iter().enumerate() {
            let target = self.root.join(&file.relative);
            let temp = self.root.join(&file.temp);
            if let Err(source) = fs::rename(&temp, &target) {
                if replaced == 0 {
                    // Nothing was replaced: discard the staged bytes and
                    // the record, so a caller sees a plain failed write
                    // instead of an interrupted change that never
                    // started.
                    discard_prepared(&self.root, &self.files);
                    let _ = fs::remove_file(&self.record_path);
                    return Err(ConfigError::Io {
                        path: target,
                        action: "replace",
                        source,
                    });
                }
                return Err(ConfigError::CommitInterrupted {
                    record: self.record_path.clone(),
                    detail: format!("replacing {} failed: {source}", file.relative),
                });
            }
            #[cfg(test)]
            crash_point(CrashAfter::Rename(replaced + 1));
        }
        // Every replacement must be durable before the record that
        // describes the change disappears; otherwise a power loss could
        // leave a silent mix of old and new files with no record.
        for directory in target_directories(&self.root, self.files.iter().map(|f| &f.relative)) {
            if let Err(error) = sync_directory(&directory) {
                return Err(ConfigError::CommitInterrupted {
                    record: self.record_path.clone(),
                    detail: format!(
                        "all files were replaced but the replacements are not known to be durable: {error}"
                    ),
                });
            }
        }
        if let Err(source) = fs::remove_file(&self.record_path) {
            return Err(ConfigError::CommitInterrupted {
                record: self.record_path.clone(),
                detail: format!(
                    "all files were replaced but the transaction record could not be removed: {source}"
                ),
            });
        }
        // Every replacement is durable already; if the removal itself is
        // lost, the record reappears describing a finished change, which
        // `--complete` resolves without touching a file. Reporting the save
        // as failed here would be wrong.
        if let Some(parent) = self.record_path.parent() {
            let _ = sync_directory(parent);
        }
        #[cfg(test)]
        crash_point(CrashAfter::RecordRemoval);
        // The backups go after the record: a record never outlives the
        // previous content it may need for a rollback.
        for file in &self.files {
            if let Some(backup) = &file.backup {
                let _ = fs::remove_file(self.root.join(backup));
            }
        }
        Ok(CommittedRevision {
            digest: self.digest,
            files: self
                .files
                .iter()
                .map(|file| file.relative.clone())
                .collect(),
        })
    }
}

/// One unresolved multi-file change as found on disk.
#[derive(Debug)]
pub struct InterruptedTransaction {
    root: PathBuf,
    record_path: PathBuf,
    state: String,
    files: Vec<RecordFile>,
    detail: String,
}

impl InterruptedTransaction {
    /// The transaction record that describes the interrupted change.
    pub fn record_path(&self) -> &Path {
        &self.record_path
    }

    /// The recorded state of the change (`prepared` for a record that
    /// was never finished).
    pub fn state(&self) -> &str {
        &self.state
    }

    /// The config-root-relative files the change spans.
    pub fn files(&self) -> Vec<&str> {
        self.files
            .iter()
            .map(|file| file.relative.as_str())
            .collect()
    }

    /// Why the record could not be interpreted, when it could not.
    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// The record's id: its file name without the `.json` suffix
    /// (`txn-<pid>-<nanos>`).
    pub fn id(&self) -> &str {
        self.record_path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_suffix(JOURNAL_SUFFIX))
            .unwrap_or_default()
    }

    /// Whether `action` can run safely on the files as they are now, and
    /// the precise reason when it cannot. [`Self::recover`] runs the same
    /// check again under the journal lock before it changes anything.
    ///
    /// # Errors
    ///
    /// The reason the recovery would be refused.
    pub fn check(&self, action: Recovery) -> Result<(), String> {
        self.plan(action).map(|_| ())
    }

    /// True when the change never replaced anything: every prepared file
    /// is still staged and every target still holds exactly its previous
    /// revision. A save only reports success after its record is removed,
    /// so such a change was never applied nor reported as saved, and a
    /// rollback only discards staged copies.
    ///
    /// This is a classification of the files as they are now; only
    /// [`Self::discard_if_untouched`] may act on it, because it decides
    /// again under the journal lock.
    pub fn is_untouched(&self) -> bool {
        self.detail.is_empty()
            && self.linked_path().is_none()
            && self.files.iter().all(|file| {
                self.root.join(&file.temp).is_file() && self.target_holds_previous_revision(file)
            })
    }

    /// Roll back the change only if it is still untouched (see
    /// [`Self::is_untouched`]), decided on the record and the files as they
    /// are under the journal lock. `Ok(None)`: the change is not (or no
    /// longer) untouched and is left for the operator, unchanged.
    ///
    /// # Errors
    ///
    /// The errors of [`Self::recover`].
    pub fn discard_if_untouched(&self) -> Result<Option<RecoveryOutcome>, ConfigError> {
        self.resolve(Recovery::Rollback, true)
    }

    /// Apply an explicit recovery decision.
    ///
    /// [`Recovery::Complete`] finishes the prepared replacements after
    /// verifying every byte, [`Recovery::Rollback`] restores the
    /// previous content from the verified backups. Every file is
    /// verified before the first one is touched, a file that already
    /// holds the chosen revision is left as it is (so a recovery that was
    /// itself interrupted can simply be run again), and a target that
    /// holds neither revision is never overwritten. Both remove the
    /// record afterwards; neither ever runs on its own.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::RecoveryFailed`] when the record is
    /// unreadable or gone, when another configuration change is being
    /// written at this moment, or when a file cannot be verified or
    /// restored, leaving the record in place.
    pub fn recover(self, action: Recovery) -> Result<RecoveryOutcome, ConfigError> {
        match self.resolve(action, false)? {
            Some(outcome) => Ok(outcome),
            None => Err(ConfigError::RecoveryFailed {
                record: self.record_path,
                detail: "the recovery made no decision".to_string(),
            }),
        }
    }

    fn resolve(
        &self,
        action: Recovery,
        only_if_untouched: bool,
    ) -> Result<Option<RecoveryOutcome>, ConfigError> {
        let failed = |detail: String| ConfigError::RecoveryFailed {
            record: self.record_path.clone(),
            detail,
        };
        if !self.detail.is_empty() {
            return Err(failed(self.detail.clone()));
        }
        let journal = self.root.join(JOURNAL_DIR);
        let lock = open_journal_lock(&journal)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(fs::TryLockError::WouldBlock) => {
                return Err(failed(
                    "another configuration change is being written right now; retry when it has finished"
                        .to_string(),
                ))
            }
            Err(fs::TryLockError::Error(source)) => {
                return Err(ConfigError::Io {
                    path: journal.join(JOURNAL_LOCK),
                    action: "lock",
                    source,
                })
            }
        }
        // Decide on the record and the files as they are under the lock:
        // the record may have been resolved or advanced since it was listed.
        if !self.record_path.is_file() {
            return Err(failed(
                "the record no longer exists; the change was already resolved".to_string(),
            ));
        }
        let current = read_record(&self.root, self.record_path.clone());
        if !current.detail.is_empty() {
            return Err(failed(current.detail));
        }
        if only_if_untouched && !current.is_untouched() {
            return Ok(None);
        }
        let steps = current.plan(action).map_err(failed)?;
        for (index, (file, step)) in current.files.iter().zip(steps).enumerate() {
            current.apply(file, step)?;
            #[cfg(test)]
            crash_point(CrashAfter::RecoveryStep(index + 1));
            #[cfg(not(test))]
            let _ = index;
        }
        // Files found already done (an earlier recovery attempt, or a commit
        // killed before its directory sync) may still sit on undurable
        // renames: every target directory is synced before the record goes.
        for directory in
            target_directories(&current.root, current.files.iter().map(|f| &f.relative))
        {
            sync_directory(&directory).map_err(|error| {
                failed(format!(
                    "the files hold the chosen revision but are not known to be durable: {error}"
                ))
            })?;
        }
        fs::remove_file(&current.record_path).map_err(|source| {
            failed(format!(
                "the change was recovered but its record could not be removed: {source}"
            ))
        })?;
        // Every file is durable already (synced just above); a lost removal only brings back a record whose chosen action
        // finds nothing left to do.
        let _ = sync_directory(&journal);
        for file in &current.files {
            if let Some(backup) = &file.backup {
                let _ = fs::remove_file(current.root.join(backup));
            }
        }
        Ok(Some(RecoveryOutcome {
            action,
            files: current
                .files
                .iter()
                .map(|file| file.relative.clone())
                .collect(),
        }))
    }

    /// The first path component below the config root, on the way to any
    /// file or the record, that is a symbolic link.
    fn linked_path(&self) -> Option<String> {
        std::iter::once(format!("{JOURNAL_DIR}/record"))
            .chain(self.files.iter().flat_map(|file| {
                [
                    Some(file.relative.clone()),
                    Some(file.temp.clone()),
                    file.backup.clone(),
                ]
                .into_iter()
                .flatten()
            }))
            .find_map(|relative| symlinked_ancestor(&self.root, &relative))
    }

    /// One verified step per file, or the reason `action` is refused.
    fn plan(&self, action: Recovery) -> Result<Vec<RecoveryStep>, String> {
        if !self.detail.is_empty() {
            return Err(format!("the record is unreadable: {}", self.detail));
        }
        if let Some(link) = self.linked_path() {
            return Err(format!(
                "{link} is a symbolic link; recovery never follows links out of the config root"
            ));
        }
        self.files
            .iter()
            .map(|file| match action {
                Recovery::Complete => self.plan_complete(file),
                Recovery::Rollback => self.plan_rollback(file),
            })
            .collect()
    }

    fn plan_complete(&self, file: &RecordFile) -> Result<RecoveryStep, String> {
        let temp = self.root.join(&file.temp);
        let target = self.root.join(&file.relative);
        if temp.is_file() {
            if digest_of_path(&temp) != Some(file.digest) {
                return Err(format!(
                    "the prepared new content of {} is damaged (it does not match the recorded digest)",
                    file.relative
                ));
            }
            if !self.target_holds_previous_revision(file) {
                return Err(format!(
                    "{} changed after the interrupted change was prepared; completing would overwrite it",
                    file.relative
                ));
            }
            return Ok(RecoveryStep::Replace);
        }
        if target.is_file() && digest_of_path(&target) == Some(file.digest) {
            return Ok(RecoveryStep::Done);
        }
        Err(format!(
            "the prepared new content of {} is gone and the file does not hold the new revision",
            file.relative
        ))
    }

    fn plan_rollback(&self, file: &RecordFile) -> Result<RecoveryStep, String> {
        let target = self.root.join(&file.relative);
        if self.root.join(&file.temp).exists() {
            // The replacement never happened: dropping the prepared
            // bytes is the whole rollback for this file.
            return Ok(RecoveryStep::DiscardTemp);
        }
        if self.target_holds_previous_revision(file) {
            return Ok(RecoveryStep::Done);
        }
        if !(target.is_file() && digest_of_path(&target) == Some(file.digest)) {
            return Err(format!(
                "{} holds neither its previous nor its new revision; refusing to overwrite it",
                file.relative
            ));
        }
        match (&file.backup, file.backup_digest) {
            (Some(backup), Some(backup_digest)) => {
                let bytes = fs::read(self.root.join(backup)).map_err(|source| {
                    format!(
                        "the previous content of {} is unreadable: {source}",
                        file.relative
                    )
                })?;
                if digest_of_bytes(&bytes) != backup_digest {
                    return Err(format!(
                        "the backup of {} is damaged (it does not match the recorded digest)",
                        file.relative
                    ));
                }
                Ok(RecoveryStep::Restore(bytes))
            }
            _ if !file.existed_before => Ok(RecoveryStep::RemoveTarget),
            _ => Err(format!(
                "{} did not hold a regular file before the change and cannot be restored",
                file.relative
            )),
        }
    }

    /// Whether the target is exactly what it was when the change was
    /// prepared.
    fn target_holds_previous_revision(&self, file: &RecordFile) -> bool {
        let target = self.root.join(&file.relative);
        match file.backup_digest {
            Some(previous) => target.is_file() && digest_of_path(&target) == Some(previous),
            None if !file.existed_before => fs::symlink_metadata(&target).is_err(),
            None => fs::symlink_metadata(&target).is_ok_and(|meta| !meta.is_file()),
        }
    }

    fn apply(&self, file: &RecordFile, step: RecoveryStep) -> Result<(), ConfigError> {
        let temp = self.root.join(&file.temp);
        let target = self.root.join(&file.relative);
        match step {
            RecoveryStep::Done => Ok(()),
            RecoveryStep::Replace => replace_with_temp(&temp, &target),
            RecoveryStep::DiscardTemp => {
                fs::remove_file(&temp).map_err(ConfigError::io(&temp, "remove"))
            }
            RecoveryStep::RemoveTarget => {
                fs::remove_file(&target).map_err(ConfigError::io(&target, "remove"))?;
                match target.parent() {
                    Some(parent) => sync_directory(parent),
                    None => Ok(()),
                }
            }
            RecoveryStep::Restore(bytes) => {
                // A fixed name next to the target: a restore interrupted
                // after staging leaves nothing behind once it is re-run.
                let staged = self.root.join(restore_scratch_relative(&file.temp));
                match fs::remove_file(&staged) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(source) => {
                        return Err(ConfigError::Io {
                            path: staged,
                            action: "remove",
                            source,
                        })
                    }
                }
                write_new_file(&staged, &bytes)?;
                replace_with_temp(&staged, &target)
            }
        }
    }
}

/// What a verified recovery does to one file.
enum RecoveryStep {
    /// The file already holds the chosen revision.
    Done,
    /// Rename the verified prepared bytes over the target.
    Replace,
    /// Drop the prepared bytes of a target that was never replaced.
    DiscardTemp,
    /// Write the verified previous content back.
    Restore(Vec<u8>),
    /// The target did not exist before the change.
    RemoveTarget,
}

/// The explicit recovery decision for an interrupted change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Finish the prepared replacements after verifying every byte.
    Complete,
    /// Restore the previous content from the verified backups.
    Rollback,
}

impl Recovery {
    /// The canonical token of this decision (log/display form).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Rollback => "rollback",
        }
    }
}

impl fmt::Display for Recovery {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// What an explicit recovery did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveryOutcome {
    pub action: Recovery,
    pub files: Vec<String>,
}

/// The unresolved transaction records under `root`, by record name.
///
/// A record whose JSON cannot be read is reported too (with its
/// `detail` explaining why) instead of being hidden or deleted: an
/// interrupted change must stay visible until it is recovered.
///
/// # Errors
///
/// Only a filesystem failure while listing the journal directory.
pub fn interrupted_transactions(root: &Path) -> Result<Vec<InterruptedTransaction>, ConfigError> {
    let journal = root.join(JOURNAL_DIR);
    if !journal.is_dir() {
        return Ok(Vec::new());
    }
    let entries = fs::read_dir(&journal).map_err(ConfigError::io(&journal, "read"))?;
    let mut records = Vec::new();
    for entry in entries {
        let entry = entry.map_err(ConfigError::io(&journal, "read"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !(name.starts_with(JOURNAL_PREFIX) && name.ends_with(JOURNAL_SUFFIX)) {
            continue;
        }
        records.push(read_record(root, entry.path()));
    }
    records.sort_by(|left, right| left.record_path.cmp(&right.record_path));
    Ok(records)
}

#[derive(Debug)]
struct PreparedFile {
    relative: String,
    /// Staging path, relative to the config root.
    temp: String,
    /// Backup path, relative to the config root (`None` when the target
    /// did not hold a regular file before the change).
    backup: Option<String>,
    existed_before: bool,
    digest: u64,
    backup_digest: Option<u64>,
}

#[derive(Debug, Clone)]
struct RecordFile {
    relative: String,
    temp: String,
    backup: Option<String>,
    existed_before: bool,
    digest: u64,
    backup_digest: Option<u64>,
}

fn stage_file(
    root: &Path,
    id: &str,
    index: usize,
    write: &FileWrite<'_>,
) -> Result<PreparedFile, ConfigError> {
    let target = root.join(write.relative_path);
    let (temp, temp_relative) = stage_paths(&target, write.relative_path, id, index)?;
    write_new_file(&temp, write.contents.as_bytes())?;

    let (existed_before, backup, backup_digest) = if target.is_file() {
        let bytes = fs::read(&target).map_err(ConfigError::io(&target, "read"))?;
        let backup_relative = backup_relative(id, index);
        let backup = root.join(&backup_relative);
        if let Err(error) = write_new_file(&backup, &bytes) {
            let _ = fs::remove_file(&temp);
            return Err(error);
        }
        (true, Some(backup_relative), Some(digest_of_bytes(&bytes)))
    } else {
        // A target that exists but is not a regular file (for example a
        // directory in its place) has no previous content to keep; the
        // replacement decides whether it can be written at all.
        (target.exists(), None, None)
    };

    Ok(PreparedFile {
        relative: write.relative_path.to_string(),
        temp: temp_relative,
        backup,
        existed_before,
        digest: digest_of_bytes(write.contents.as_bytes()),
        backup_digest,
    })
}

/// The absolute staging path of `relative_path` plus its
/// config-root-relative spelling for the transaction record. Missing
/// parent directories are created first (the upstream savers call
/// `Directory.CreateDirectory` on every write).
fn stage_paths(
    target: &Path,
    relative_path: &str,
    id: &str,
    index: usize,
) -> Result<(PathBuf, String), ConfigError> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(ConfigError::io(parent, "create"))?;
    }
    let name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ConfigError::malformed(target, "target has no usable file name"))?;
    let temp = target.with_file_name(format!(".{name}.{id}.{index}.tmp"));
    Ok((temp, staged_relative(relative_path, id, index)))
}

/// The staging path `stage_paths` generates for entry `index` of
/// transaction `id`, relative to the config root.
fn staged_relative(relative_path: &str, id: &str, index: usize) -> String {
    match relative_path.rsplit_once('/') {
        Some((directory, name)) => format!("{directory}/.{name}.{id}.{index}.tmp"),
        None => format!(".{relative_path}.{id}.{index}.tmp"),
    }
}

/// The scratch file a rollback's restore step writes before renaming it
/// over the target, relative to the config root (derived from the entry's
/// staged path).
fn restore_scratch_relative(staged: &str) -> String {
    format!("{staged}.restore")
}

/// The backup path `stage_file` generates for entry `index` of
/// transaction `id`, relative to the config root.
fn backup_relative(id: &str, index: usize) -> String {
    format!("{JOURNAL_DIR}/backup-{id}-{index}")
}

/// The transaction id (`<pid>-<nanos>`) a record file name carries, when
/// it has exactly the generated shape `txn-<pid>-<nanos>.json`.
fn record_transaction_id(record_path: &Path) -> Option<&str> {
    let id = record_path
        .file_name()?
        .to_str()?
        .strip_prefix(JOURNAL_PREFIX)?
        .strip_suffix(JOURNAL_SUFFIX)?;
    let (pid, nanos) = id.split_once('-')?;
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    (digits(pid) && digits(nanos)).then_some(id)
}

/// Whether `path` is `ancestor` or lies below it (lexically).
fn is_same_or_below(path: &str, ancestor: &str) -> bool {
    path.strip_prefix(ancestor)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// The first component of `relative`'s parent directories below `root`
/// that is a symbolic link.
fn symlinked_ancestor(root: &Path, relative: &str) -> Option<String> {
    let (ancestors, _) = relative.rsplit_once('/')?;
    let mut prefix = String::new();
    for part in ancestors.split('/') {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(part);
        if fs::symlink_metadata(root.join(&prefix)).is_ok_and(|meta| meta.file_type().is_symlink())
        {
            return Some(prefix);
        }
    }
    None
}

// Configuration roots are caller-owned. Journal entries must use unambiguous
// relative paths; absolute paths and parent traversal are never recovered.
fn validate_relative_path(relative: &str) -> Result<(), ConfigError> {
    if relative
        .split('/')
        .any(|part| matches!(part, "" | "." | ".."))
        || relative.contains('\\')
        || !Path::new(relative)
            .components()
            .all(|part| matches!(part, std::path::Component::Normal(_)))
    {
        return Err(ConfigError::malformed(
            relative,
            "expected a relative path without traversal",
        ));
    }
    Ok(())
}

/// A configuration file a write may target: a safe relative path that is
/// neither the journal nor inside it.
fn validate_target_path(relative: &str) -> Result<(), ConfigError> {
    validate_relative_path(relative)?;
    if is_same_or_below(relative, JOURNAL_DIR) || is_same_or_below(JOURNAL_DIR, relative) {
        return Err(ConfigError::malformed(
            relative,
            "the transaction journal is not a configuration file",
        ));
    }
    Ok(())
}

fn record_json(files: &[PreparedFile]) -> String {
    let entries: Vec<Value> = files
        .iter()
        .map(|file| {
            json!({
                "relative": file.relative,
                "temp": file.temp,
                "backup": file.backup,
                "existed_before": file.existed_before,
                "digest": file.digest,
                "backup_digest": file.backup_digest,
            })
        })
        .collect();
    json!({
        "state": "prepared",
        "files": entries,
    })
    .to_string()
}

fn read_record(root: &Path, record_path: PathBuf) -> InterruptedTransaction {
    let unreadable = |detail: String| InterruptedTransaction {
        root: root.to_path_buf(),
        record_path: record_path.clone(),
        state: "unreadable".to_string(),
        files: Vec::new(),
        detail,
    };
    let Some(id) = record_transaction_id(&record_path).map(str::to_string) else {
        return unreadable(
            "the record name is not a generated transaction record name".to_string(),
        );
    };
    let text = match fs::read_to_string(&record_path) {
        Ok(text) => text,
        Err(source) => return unreadable(format!("record could not be read: {source}")),
    };
    let document: Value = match serde_json::from_str(&text) {
        Ok(document) => document,
        Err(source) => return unreadable(format!("record is not valid JSON: {source}")),
    };
    let state = document
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    let Some(entries) = document.get("files").and_then(Value::as_array) else {
        return unreadable("record lists no files".to_string());
    };
    let mut files = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(relative) = entry.get("relative").and_then(Value::as_str) else {
            return unreadable("record entry without a relative path".to_string());
        };
        let Some(temp) = entry.get("temp").and_then(Value::as_str) else {
            return unreadable("record entry without a prepared file".to_string());
        };
        let Some(digest) = entry.get("digest").and_then(Value::as_u64) else {
            return unreadable("record entry without a digest".to_string());
        };
        for path in [
            Some(relative),
            Some(temp),
            entry.get("backup").and_then(Value::as_str),
        ]
        .into_iter()
        .flatten()
        {
            if let Err(error) = validate_relative_path(path) {
                return unreadable(error.to_string());
            }
        }
        if files
            .iter()
            .any(|file: &RecordFile| file.relative == relative)
        {
            return unreadable("duplicate transaction target".to_string());
        }
        let Some(existed_before) = entry.get("existed_before").and_then(Value::as_bool) else {
            return unreadable("record entry without its previous-file state".to_string());
        };
        let backup_digest = entry.get("backup_digest").and_then(Value::as_u64);
        let backup = entry.get("backup").and_then(Value::as_str);
        if backup.is_some() != backup_digest.is_some() {
            return unreadable(
                "record entry with a backup but no digest (or the reverse)".to_string(),
            );
        }
        if backup.is_some() && !existed_before {
            return unreadable(
                "record entry with a backup of a file that did not exist".to_string(),
            );
        }
        // Every path must have exactly the role and shape this record's
        // own transaction generated; anything else could make a recovery
        // step act on the wrong file (a "staged copy" that is the target).
        if let Err(error) = validate_target_path(relative) {
            return unreadable(error.to_string());
        }
        if temp != staged_relative(relative, &id, index) {
            return unreadable(format!(
                "the staged path of {relative} is not the one this transaction generated"
            ));
        }
        if backup.is_some_and(|backup| backup != backup_relative(&id, index)) {
            return unreadable(format!(
                "the backup path of {relative} is not the one this transaction generated"
            ));
        }
        files.push(RecordFile {
            relative: relative.to_string(),
            temp: temp.to_string(),
            backup: backup.map(str::to_string),
            existed_before,
            digest,
            backup_digest,
        });
    }
    if files.is_empty() {
        return unreadable("record lists no files".to_string());
    }
    // Every path a recovery may read, write, rename or delete: the target,
    // the staged copy, the backup and the restore scratch file. None may
    // equal or contain another, or one step would act on another's file.
    let paths: Vec<String> = files
        .iter()
        .flat_map(|file| {
            [
                Some(file.relative.clone()),
                Some(file.temp.clone()),
                file.backup.clone(),
                Some(restore_scratch_relative(&file.temp)),
            ]
        })
        .flatten()
        .collect();
    for (left_index, left) in paths.iter().enumerate() {
        for (right_index, right) in paths.iter().enumerate() {
            if left_index != right_index && is_same_or_below(right.as_str(), left.as_str()) {
                return unreadable(format!("the record paths {left} and {right} alias"));
            }
        }
    }
    InterruptedTransaction {
        root: root.to_path_buf(),
        record_path,
        state,
        files,
        detail: String::new(),
    }
}

fn unresolved_record_for(root: &Path, relative_path: &str) -> Result<Option<PathBuf>, ConfigError> {
    for record in interrupted_transactions(root)? {
        if record.detail.is_empty() {
            if record
                .files
                .iter()
                .any(|file| file.relative == relative_path)
            {
                return Ok(Some(record.record_path));
            }
        } else {
            // An unreadable record blocks every write: its files are
            // unknown, so any replacement could silently adopt it.
            return Ok(Some(record.record_path));
        }
    }
    Ok(None)
}

fn first_record(journal: &Path) -> Result<Option<PathBuf>, ConfigError> {
    if !journal.is_dir() {
        return Ok(None);
    }
    let entries = fs::read_dir(journal).map_err(ConfigError::io(journal, "read"))?;
    for entry in entries {
        let entry = entry.map_err(ConfigError::io(journal, "read"))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(JOURNAL_PREFIX) && name.ends_with(JOURNAL_SUFFIX) {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// Create `path` exclusively, write `bytes`, and sync them to disk.
fn write_new_file(path: &Path, bytes: &[u8]) -> Result<(), ConfigError> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(ConfigError::io(path, "create"))?;
    if let Err(source) = file.write_all(bytes).and_then(|()| file.sync_all()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(ConfigError::Io {
            path: path.to_path_buf(),
            action: "write",
            source,
        });
    }
    Ok(())
}

fn replace_with_temp(temp: &Path, target: &Path) -> Result<(), ConfigError> {
    fs::rename(temp, target).map_err(ConfigError::io(target, "replace"))?;
    match target.parent() {
        Some(parent) => sync_directory(parent),
        None => Ok(()),
    }
}

fn discard_prepared(root: &Path, files: &[PreparedFile]) {
    for file in files {
        let _ = fs::remove_file(root.join(&file.temp));
        if let Some(backup) = &file.backup {
            let _ = fs::remove_file(root.join(backup));
        }
    }
}

/// Create the journal and take its lock, waiting for a holder in another
/// thread or process to finish (see the module docs).
fn lock_journal(root: &Path) -> Result<fs::File, ConfigError> {
    let journal = root.join(JOURNAL_DIR);
    fs::create_dir_all(&journal).map_err(ConfigError::io(&journal, "create"))?;
    let lock = open_journal_lock(&journal)?;
    lock.lock()
        .map_err(ConfigError::io(journal.join(JOURNAL_LOCK), "lock"))?;
    Ok(lock)
}

/// The journal's advisory lock file (see the module docs); the caller
/// locks it.
fn open_journal_lock(journal: &Path) -> Result<fs::File, ConfigError> {
    let path = journal.join(JOURNAL_LOCK);
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(ConfigError::io(path, "open"))
}

/// The distinct parent directories of `relative` targets under `root`.
fn target_directories<'a>(root: &Path, relative: impl Iterator<Item = &'a String>) -> Vec<PathBuf> {
    let mut directories: Vec<PathBuf> = Vec::new();
    for relative in relative {
        if let Some(parent) = root.join(relative).parent() {
            if !directories.iter().any(|known| known == parent) {
                directories.push(parent.to_path_buf());
            }
        }
    }
    directories
}

/// Every directory holding a staged file or a backup of `files`.
fn staging_directories(root: &Path, files: &[PreparedFile]) -> Vec<PathBuf> {
    let mut directories = target_directories(root, files.iter().map(|file| &file.relative));
    directories.push(root.join(JOURNAL_DIR));
    directories
}

/// Make the entries of `path` (renames, creations, removals) durable.
fn sync_directory(path: &Path) -> Result<(), ConfigError> {
    #[cfg(test)]
    if FAIL_DIRECTORY_SYNC.with(std::cell::Cell::get) {
        return Err(ConfigError::Io {
            path: path.to_path_buf(),
            action: "sync",
            source: std::io::Error::other("simulated directory sync failure"),
        });
    }
    fs::File::open(path)
        .and_then(|handle| handle.sync_all())
        .map_err(ConfigError::io(path, "sync"))
}

fn transaction_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos}", std::process::id())
}

fn digest_of_path(path: &Path) -> Option<u64> {
    fs::read(path).ok().map(|bytes| digest_of_bytes(&bytes))
}

fn digest_of_bytes(bytes: &[u8]) -> u64 {
    let mut digest = FNV_OFFSET_BASIS;
    digest_bytes(&mut digest, bytes);
    digest
}

fn digest_of_files<'a>(files: impl IntoIterator<Item = (String, &'a [u8])>) -> u64 {
    let mut digest = FNV_OFFSET_BASIS;
    for (relative, bytes) in files {
        digest_bytes(&mut digest, relative.as_bytes());
        digest_bytes(&mut digest, &[0]);
        digest_bytes(&mut digest, bytes);
    }
    digest
}

fn digest_bytes(digest: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *digest ^= u64::from(*byte);
        *digest = digest.wrapping_mul(FNV_PRIME);
    }
}

/// Transaction boundaries a test can stop at as if the process were
/// killed there: [`crash_point`] unwinds without running any of the
/// cleanup an ordinary error path would.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CrashAfter {
    /// Every file and backup is staged and synced, no record exists yet.
    Staging,
    /// The record is published, no target is replaced yet.
    Record,
    /// The n-th target (1-based) was just replaced.
    Rename(usize),
    /// Every target is replaced and the record removed; backups remain.
    RecordRemoval,
    /// A recovery finished its n-th file (1-based); the record remains.
    RecoveryStep(usize),
}

#[cfg(test)]
thread_local! {
    static CRASH_AT: std::cell::Cell<Option<CrashAfter>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
thread_local! {
    static FAIL_DIRECTORY_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static PAUSE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

/// Make every directory sync on this thread fail until disarmed.
#[cfg(test)]
pub(crate) fn fail_directory_sync(fail: bool) {
    FAIL_DIRECTORY_SYNC.with(|cell| cell.set(fail));
}

/// Run `pause` the next time this thread's single-file write is about to
/// rename its staged file over the target.
#[cfg(test)]
pub(crate) fn pause_before_replace(pause: Box<dyn FnOnce()>) {
    PAUSE.with(|cell| *cell.borrow_mut() = Some(pause));
}

#[cfg(test)]
fn pause_point() {
    if let Some(pause) = PAUSE.with(|cell| cell.borrow_mut().take()) {
        pause();
    }
}

/// Arm [`crash_point`] for the next time this thread reaches `point`.
#[cfg(test)]
pub(crate) fn crash_at(point: CrashAfter) {
    CRASH_AT.with(|cell| cell.set(Some(point)));
}

/// Marker payload of a simulated crash.
#[cfg(test)]
pub(crate) struct SimulatedCrash;

#[cfg(test)]
fn crash_point(point: CrashAfter) {
    if CRASH_AT.with(|cell| cell.get()) == Some(point) {
        CRASH_AT.with(|cell| cell.set(None));
        std::panic::panic_any(SimulatedCrash);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use std::env;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(test_name: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos().to_string())
            .unwrap_or_else(|_| "time_error".to_string());
        let root = env::temp_dir().join(format!("d4lc-persistence-{test_name}-{suffix}"));
        fs::create_dir_all(&root).expect("create test root");
        root
    }

    fn staging_files(root: &Path) -> Vec<PathBuf> {
        fs::read_dir(root.join("Config"))
            .expect("read Config")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.to_string_lossy().ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn writes_reject_paths_outside_the_config_root_before_staging() {
        let sandbox = temp_root("unsafe_write_paths");
        let root = sandbox.join("root");
        let outside = sandbox.join("escape.json");
        fs::write(&outside, "keep").expect("seed outside file");
        for path in [
            "../escape.json",
            outside.to_str().expect("path"),
            "Config/../escape.json",
            "",
        ] {
            assert!(
                write_atomic(&root, path, "new").is_err(),
                "accepted {path:?}"
            );
        }
        assert_eq!(fs::read_to_string(&outside).expect("read outside"), "keep");
        assert!(!root.exists(), "invalid paths must not be staged");
        fs::remove_dir_all(sandbox).expect("clean root");
    }

    #[test]
    fn recovery_rejects_unsafe_record_paths_without_touching_files() {
        let root = temp_root("unsafe_recovery_paths");
        let outside = root.with_extension("outside");
        fs::write(&outside, "keep").expect("seed outside file");
        let journal = root.join(JOURNAL_DIR);
        fs::create_dir_all(&journal).expect("create journal");
        for field in ["relative", "temp", "backup"] {
            let mut entry = json!({
                "relative": "Config/Settings.json", "temp": "Config/.settings.tmp",
                "backup": "Config/.persistence/backup-test", "existed_before": false,
                "digest": 0, "backup_digest": 0,
            });
            entry[field] = json!(outside.to_str().expect("path"));
            let record = journal.join("txn-invalid.json");
            fs::write(
                &record,
                json!({"state": "prepared", "files": [entry]}).to_string(),
            )
            .expect("write record");
            for action in [Recovery::Complete, Recovery::Rollback] {
                let mut records = interrupted_transactions(&root).expect("list records");
                let pending = records.pop().expect("one record");
                assert!(!pending.detail().is_empty(), "accepted unsafe {field}");
                assert!(pending.recover(action).is_err());
                assert!(record.exists());
                assert_eq!(fs::read_to_string(&outside).expect("read outside"), "keep");
            }
        }
        fs::remove_file(outside).expect("remove outside file");
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn transactions_reject_duplicate_targets_before_staging() {
        let root = temp_root("duplicate_targets");
        let writes = [
            FileWrite {
                relative_path: "Config/A.json",
                contents: "first",
            },
            FileWrite {
                relative_path: "Config/A.json",
                contents: "second",
            },
        ];
        assert!(prepare_transaction(&root, &writes).is_err());
        assert!(!root.join("Config").exists());
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn atomic_write_replaces_the_target_and_reports_a_revision() {
        let root = temp_root("atomic_write");
        let first = write_atomic(&root, "Config/Settings.json", "{\"a\":1}")
            .expect("first write should succeed");
        let second = write_atomic(&root, "Config/Settings.json", "{\"a\":2}")
            .expect("second write should succeed");

        assert_eq!(
            fs::read_to_string(root.join("Config/Settings.json")).expect("read target"),
            "{\"a\":2}"
        );
        assert_ne!(
            first.digest, second.digest,
            "a content change is a new revision"
        );
        assert_eq!(second.files, vec!["Config/Settings.json".to_string()]);
        assert!(
            staging_files(&root).is_empty(),
            "staging files must be gone: {:?}",
            staging_files(&root)
        );
        assert!(interrupted_transactions(&root).expect("list").is_empty());

        fs::remove_dir_all(&root).expect("clean test root");
    }

    #[test]
    fn write_failure_before_replacement_keeps_the_previous_content() {
        let root = temp_root("write_failure_before_replacement");
        let revision = write_atomic(&root, "Config/Settings.json", "previous")
            .expect("initial write should succeed");

        // A directory where the file is expected cannot be replaced by a
        // rename; the failure happens after the bytes were staged.
        let target = root.join("Config/AffixPresets-v2.json");
        fs::create_dir_all(&target).expect("create directory in place of the file");
        let error = write_atomic(&root, "Config/AffixPresets-v2.json", "[]")
            .expect_err("replacing a directory must fail");
        assert!(
            matches!(error, ConfigError::Io { .. }),
            "unexpected: {error}"
        );

        assert_eq!(
            fs::read_to_string(root.join("Config/Settings.json")).expect("read unchanged target"),
            "previous"
        );
        let unchanged = write_atomic(&root, "Config/Settings.json", "previous")
            .expect("rewrite should succeed");
        assert_eq!(
            revision, unchanged,
            "a failed write is not a new applied revision"
        );
        assert!(
            staging_files(&root).is_empty(),
            "a failed replacement must clean its staging file"
        );

        fs::remove_dir_all(&root).expect("clean test root");
    }

    #[test]
    fn transaction_failure_between_replacements_is_detected_and_recoverable() {
        let root = temp_root("transaction_between_replacements");
        write_atomic(&root, "Config/Settings.json", "old settings").expect("seed settings");
        write_atomic(&root, "Config/AffixPresets-v2.json", "old presets").expect("seed presets");

        // The second target becomes a directory after the change was
        // prepared, so the first replacement succeeds and the second
        // fails: the exact between-replacements failure the record must
        // expose.
        let settings_target = root.join("Config/Settings.json");
        fs::remove_file(&settings_target).expect("remove settings file");
        fs::create_dir_all(&settings_target).expect("directory in place of settings");

        let prepared = prepare_transaction(
            &root,
            &[
                FileWrite {
                    relative_path: "Config/AffixPresets-v2.json",
                    contents: "new presets",
                },
                FileWrite {
                    relative_path: "Config/Settings.json",
                    contents: "new settings",
                },
            ],
        )
        .expect("prepare should stage both files");
        let error = prepared
            .commit()
            .expect_err("the second replacement must fail");
        assert!(
            matches!(error, ConfigError::CommitInterrupted { .. }),
            "unexpected: {error}"
        );
        assert_eq!(
            fs::read_to_string(root.join("Config/AffixPresets-v2.json")).expect("read presets"),
            "new presets",
            "the first replacement is a partial state, not a silent success"
        );

        let detected = interrupted_transactions(&root).expect("detect interrupted change");
        assert_eq!(detected.len(), 1);
        assert_eq!(
            detected[0].files(),
            vec!["Config/AffixPresets-v2.json", "Config/Settings.json"]
        );
        assert_eq!(detected[0].state(), "prepared");
        // A fresh write refuses to adopt the partial state silently.
        let refused = write_atomic(&root, "Config/AffixPresets-v2.json", "third presets")
            .expect_err("an unresolved change blocks a rewrite");
        assert!(matches!(refused, ConfigError::PendingRecovery { .. }));

        // Rollback restores the previous bytes of the replaced file and
        // clears the record; the untouched file keeps its state.
        let outcome = detected
            .into_iter()
            .next()
            .expect("one record")
            .recover(Recovery::Rollback)
            .expect("rollback should succeed");
        assert_eq!(outcome.action, Recovery::Rollback);
        assert_eq!(
            fs::read_to_string(root.join("Config/AffixPresets-v2.json")).expect("read presets"),
            "old presets"
        );
        assert!(interrupted_transactions(&root).expect("list").is_empty());

        fs::remove_dir_all(&root).expect("clean test root");
    }

    #[test]
    fn transaction_failure_before_any_replacement_leaves_no_record() {
        let root = temp_root("transaction_before_any_replacement");
        write_atomic(&root, "Config/Settings.json", "old settings").expect("seed settings");
        // The FIRST target is a directory, so the change fails before any
        // replacement: nothing is partial, no record is left behind.
        write_atomic(&root, "Config/AffixPresets-v2.json", "old presets").expect("seed presets");
        let presets_target = root.join("Config/AffixPresets-v2.json");
        fs::remove_file(&presets_target).expect("remove presets file");
        fs::create_dir_all(&presets_target).expect("directory in place of presets");

        let error = prepare_transaction(
            &root,
            &[
                FileWrite {
                    relative_path: "Config/AffixPresets-v2.json",
                    contents: "new presets",
                },
                FileWrite {
                    relative_path: "Config/Settings.json",
                    contents: "new settings",
                },
            ],
        )
        .expect("prepare")
        .commit()
        .expect_err("the first replacement must fail");
        assert!(
            matches!(error, ConfigError::Io { .. }),
            "unexpected: {error}"
        );
        assert!(
            interrupted_transactions(&root).expect("list").is_empty(),
            "a change that replaced nothing leaves no interrupted record"
        );
        assert_eq!(
            fs::read_to_string(root.join("Config/Settings.json")).expect("read settings"),
            "old settings",
            "the second file is untouched"
        );
        assert!(
            staging_files(&root).is_empty(),
            "staging files must be gone"
        );

        fs::remove_dir_all(&root).expect("clean test root");
    }

    #[test]
    fn transaction_complete_finishes_an_interrupted_change() {
        let root = temp_root("transaction_complete");
        write_atomic(&root, "Config/A.json", "old a").expect("seed a");

        let prepared = prepare_transaction(
            &root,
            &[
                FileWrite {
                    relative_path: "Config/A.json",
                    contents: "new a",
                },
                FileWrite {
                    relative_path: "Config/B.json",
                    contents: "new b",
                },
            ],
        )
        .expect("prepare");
        // The obstruction appears after the change was prepared.
        fs::create_dir_all(root.join("Config/B.json")).expect("directory in place of b");
        prepared.commit().expect_err("second replacement must fail");
        // ... and disappears before the explicit recovery.
        fs::remove_dir_all(root.join("Config/B.json")).expect("remove obstruction");

        let detected = interrupted_transactions(&root).expect("detect");
        assert_eq!(detected.len(), 1);
        detected
            .into_iter()
            .next()
            .expect("one record")
            .recover(Recovery::Complete)
            .expect("complete should succeed");

        assert_eq!(
            fs::read_to_string(root.join("Config/A.json")).expect("read a"),
            "new a"
        );
        assert_eq!(
            fs::read_to_string(root.join("Config/B.json")).expect("read b"),
            "new b"
        );
        assert!(interrupted_transactions(&root).expect("list").is_empty());
        assert!(staging_files(&root).is_empty());

        fs::remove_dir_all(&root).expect("clean test root");
    }

    // ── Crash boundaries ────────────────────────────────────────────
    //
    // Every test below stops a transaction (or a recovery) at one
    // boundary as if the process had been killed there, then runs the
    // whole operator path: detect → recover (each applicable decision,
    // on its own copy of the crashed root) → a new save succeeds → every
    // file holds exactly the old or exactly the new revision.

    const CRASH_A: &str = "Config/AffixPresets-v2.json";
    const CRASH_S: &str = "Config/Settings.json";
    const CRASH_C: &str = "Config/Created.json";
    const OLD: [Option<&str>; 3] = [Some("old a"), Some("old s"), None];
    const NEW: [Option<&str>; 3] = [Some("new a"), Some("new s"), Some("new c")];
    const THIRD: [Option<&str>; 3] = [Some("third a"), Some("third s"), Some("third c")];

    fn crash_writes(contents: [Option<&'static str>; 3]) -> Vec<FileWrite<'static>> {
        [CRASH_A, CRASH_S, CRASH_C]
            .into_iter()
            .zip(contents)
            .map(|(relative_path, contents)| FileWrite {
                relative_path,
                contents: contents.expect("every crash write has contents"),
            })
            .collect()
    }

    fn seeded_crash_root(name: &str) -> PathBuf {
        let root = temp_root(name);
        write_atomic(&root, CRASH_A, "old a").expect("seed a");
        write_atomic(&root, CRASH_S, "old s").expect("seed s");
        root
    }

    fn file_state(root: &Path) -> [Option<String>; 3] {
        [CRASH_A, CRASH_S, CRASH_C].map(|relative| fs::read_to_string(root.join(relative)).ok())
    }

    fn assert_revision(root: &Path, expected: [Option<&str>; 3], context: &str) {
        let actual = file_state(root);
        let actual: Vec<Option<&str>> = actual.iter().map(Option::as_deref).collect();
        assert_eq!(actual, expected.to_vec(), "{context}");
    }

    /// Run `operation` until the armed crash point unwinds it.
    fn crash<T>(point: CrashAfter, operation: impl FnOnce() -> T) {
        crash_at(point);
        let payload = std::panic::catch_unwind(std::panic::AssertUnwindSafe(operation))
            .err()
            .unwrap_or_else(|| panic!("{point:?} was never reached"));
        assert!(
            payload.downcast_ref::<SimulatedCrash>().is_some(),
            "a real panic, not the simulated crash at {point:?}"
        );
    }

    fn crash_transaction(root: &Path, point: CrashAfter) {
        crash(point, || {
            prepare_transaction(root, &crash_writes(NEW)).and_then(PreparedTransaction::commit)
        });
    }

    fn copy_tree(from: &Path, to: &Path) {
        fs::create_dir_all(to).expect("create copy");
        for entry in fs::read_dir(from).expect("read tree") {
            let entry = entry.expect("entry");
            let target = to.join(entry.file_name());
            if entry.file_type().expect("type").is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).expect("copy file");
            }
        }
    }

    fn only_record(root: &Path) -> InterruptedTransaction {
        let mut records = interrupted_transactions(root).expect("detect");
        assert_eq!(records.len(), 1, "exactly one interrupted change");
        records.pop().expect("one record")
    }

    /// A new multi-file save and a single-file save both succeed and
    /// leave exactly their bytes.
    fn assert_saves_work_again(root: &Path) {
        prepare_transaction(root, &crash_writes(THIRD))
            .and_then(PreparedTransaction::commit)
            .expect("a new transaction succeeds after recovery");
        assert_revision(root, THIRD, "the new transaction is applied");
        write_atomic(root, CRASH_S, "fourth s").expect("a single-file save succeeds");
        assert!(interrupted_transactions(root).expect("list").is_empty());
    }

    /// Recover a copy of `crashed` with `action`, expecting `expected`.
    fn recover_copy(crashed: &Path, action: Recovery, expected: [Option<&str>; 3]) {
        let copy = crashed.with_extension(action.as_str());
        copy_tree(crashed, &copy);
        let record = only_record(&copy);
        assert_eq!(record.check(action), Ok(()), "{action} must be possible");
        let outcome = record.recover(action).expect("recovery succeeds");
        assert_eq!(outcome.files, vec![CRASH_A, CRASH_S, CRASH_C]);
        assert_revision(&copy, expected, &format!("after {action}"));
        assert_saves_work_again(&copy);
        fs::remove_dir_all(copy).expect("clean copy");
    }

    #[test]
    fn crash_after_staging_leaves_no_record_and_the_old_revision() {
        let root = seeded_crash_root("crash_after_staging");
        crash_transaction(&root, CrashAfter::Staging);
        assert!(interrupted_transactions(&root).expect("detect").is_empty());
        assert_revision(&root, OLD, "nothing was replaced");
        assert_saves_work_again(&root);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn crash_after_record_is_untouched_and_recoverable_both_ways() {
        let root = seeded_crash_root("crash_after_record");
        crash_transaction(&root, CrashAfter::Record);
        assert_revision(&root, OLD, "nothing was replaced");
        let record = only_record(&root);
        assert!(record.is_untouched());
        assert!(matches!(
            write_atomic(&root, CRASH_A, "blocked"),
            Err(ConfigError::PendingRecovery { .. })
        ));
        recover_copy(&root, Recovery::Complete, NEW);
        recover_copy(&root, Recovery::Rollback, OLD);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn crash_after_each_rename_is_recoverable_both_ways() {
        for renamed in 1..=3 {
            let root = seeded_crash_root(&format!("crash_after_rename_{renamed}"));
            crash_transaction(&root, CrashAfter::Rename(renamed));
            let mut partial = OLD;
            partial[..renamed].copy_from_slice(&NEW[..renamed]);
            assert_revision(
                &root,
                partial,
                "the replaced prefix carries the new revision",
            );
            let record = only_record(&root);
            assert!(!record.is_untouched(), "rename {renamed} touched a target");
            assert!(matches!(
                prepare_transaction(&root, &crash_writes(THIRD)),
                Err(ConfigError::PendingRecovery { .. })
            ));
            recover_copy(&root, Recovery::Complete, NEW);
            recover_copy(&root, Recovery::Rollback, OLD);
            fs::remove_dir_all(root).expect("clean root");
        }
    }

    #[test]
    fn crash_after_record_removal_is_a_complete_change() {
        let root = seeded_crash_root("crash_after_record_removal");
        crash_transaction(&root, CrashAfter::RecordRemoval);
        assert!(interrupted_transactions(&root).expect("detect").is_empty());
        assert_revision(&root, NEW, "the change is complete");
        assert_saves_work_again(&root);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn interrupted_recoveries_can_be_run_again_until_they_finish() {
        for action in [Recovery::Complete, Recovery::Rollback] {
            let expected = match action {
                Recovery::Complete => NEW,
                Recovery::Rollback => OLD,
            };
            let root = seeded_crash_root(&format!("crash_in_recovery_{action}"));
            crash_transaction(&root, CrashAfter::Rename(1));
            // Killed after the first file, and again after the second.
            for step in 1..=2 {
                crash(CrashAfter::RecoveryStep(step), || {
                    only_record(&root).recover(action)
                });
                let record = only_record(&root);
                assert_eq!(record.check(action), Ok(()), "{action} can run again");
            }
            // An interrupted rollback already restored previous content
            // whose new bytes are gone: completing is refused, not mixed.
            if action == Recovery::Rollback {
                let before = file_state(&root);
                let record = only_record(&root);
                assert!(record.check(Recovery::Complete).is_err());
                assert!(record.recover(Recovery::Complete).is_err());
                assert_eq!(
                    file_state(&root),
                    before,
                    "a refused recovery changes nothing"
                );
            }
            recover_copy(&root, action, expected);
            fs::remove_dir_all(root).expect("clean root");
        }
    }

    #[test]
    fn a_damaged_staged_file_allows_only_rollback() {
        let root = seeded_crash_root("damaged_staged_file");
        crash_transaction(&root, CrashAfter::Rename(1));
        let record = only_record(&root);
        let staged = root.join(&record.files[1].temp);
        fs::write(&staged, "new s, truncated").expect("damage staged file");
        let reason = record
            .check(Recovery::Complete)
            .expect_err("complete refused");
        assert!(reason.contains("damaged"), "{reason}");
        assert!(record.recover(Recovery::Complete).is_err());
        assert_revision(
            &root,
            [Some("new a"), Some("old s"), None],
            "nothing changed",
        );
        recover_copy(&root, Recovery::Rollback, OLD);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn a_damaged_or_missing_backup_allows_only_complete() {
        for missing in [false, true] {
            let root = seeded_crash_root(&format!("damaged_backup_{missing}"));
            crash_transaction(&root, CrashAfter::Rename(3));
            let record = only_record(&root);
            let backup = root.join(record.files[0].backup.as_ref().expect("backup"));
            if missing {
                fs::remove_file(&backup).expect("remove backup");
            } else {
                fs::write(&backup, "old a?").expect("damage backup");
            }
            assert!(record.check(Recovery::Rollback).is_err());
            assert!(record.recover(Recovery::Rollback).is_err());
            assert_revision(&root, NEW, "a refused rollback changes nothing");
            recover_copy(&root, Recovery::Complete, NEW);
            fs::remove_dir_all(root).expect("clean root");
        }
    }

    #[test]
    fn a_target_holding_neither_revision_is_never_overwritten() {
        let root = seeded_crash_root("foreign_target");
        crash_transaction(&root, CrashAfter::Rename(1));
        fs::write(root.join(CRASH_A), "edited by hand").expect("edit replaced file");
        let record = only_record(&root);
        assert!(record.check(Recovery::Rollback).is_err());
        assert!(record.check(Recovery::Complete).is_err());
        assert!(only_record(&root).recover(Recovery::Rollback).is_err());
        assert_eq!(
            fs::read_to_string(root.join(CRASH_A)).expect("read a"),
            "edited by hand"
        );
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        fs::remove_dir_all(root).expect("clean root");

        // A file that was never replaced but edited since: completing
        // would overwrite the edit, rolling back only drops the staged copy.
        let root = seeded_crash_root("foreign_unreplaced_target");
        crash_transaction(&root, CrashAfter::Rename(1));
        fs::write(root.join(CRASH_S), "edited by hand").expect("edit staged target");
        let record = only_record(&root);
        let reason = record
            .check(Recovery::Complete)
            .expect_err("complete refused");
        assert!(reason.contains("changed after"), "{reason}");
        record
            .recover(Recovery::Rollback)
            .expect("rollback succeeds");
        assert_revision(
            &root,
            [Some("old a"), Some("edited by hand"), None],
            "rollback restores a and keeps the edit",
        );
        assert_saves_work_again(&root);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn recovery_waits_for_no_one_while_a_change_is_being_written() {
        let root = seeded_crash_root("journal_busy");
        crash_transaction(&root, CrashAfter::Rename(1));
        let lock = open_journal_lock(&root.join(JOURNAL_DIR)).expect("open lock");
        lock.lock().expect("hold the journal lock");
        let error = only_record(&root)
            .recover(Recovery::Rollback)
            .expect_err("a busy journal refuses recovery");
        assert!(error.to_string().contains("being written"), "{error}");
        drop(lock);
        recover_copy(&root, Recovery::Rollback, OLD);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn an_unreadable_record_is_reported_and_never_removed() {
        let root = seeded_crash_root("torn_record");
        let journal = root.join(JOURNAL_DIR);
        fs::create_dir_all(&journal).expect("journal");
        let record_path = journal.join("txn-torn.json");
        fs::write(&record_path, "{\"state\":\"prep").expect("torn record");
        let record = only_record(&root);
        assert_eq!(record.id(), "txn-torn");
        assert!(!record.detail().is_empty());
        for action in [Recovery::Complete, Recovery::Rollback] {
            assert!(record.check(action).is_err());
        }
        assert!(record.recover(Recovery::Rollback).is_err());
        assert!(record_path.is_file());
        assert!(write_atomic(&root, CRASH_S, "blocked").is_err());
        fs::remove_dir_all(root).expect("clean root");
    }

    /// A record entry as a crafted or corrupted journal could hold it.
    fn record_entry(relative: &str, temp: &str, backup: Option<&str>, previous: &[u8]) -> Value {
        json!({
            "relative": relative, "temp": temp, "backup": backup,
            "existed_before": true, "digest": digest_of_bytes(b"new"),
            "backup_digest": backup.map(|_| digest_of_bytes(previous)),
        })
    }

    #[test]
    fn records_with_aliased_or_foreign_paths_are_unreadable_and_never_acted_on() {
        let id = "7-11";
        let generated_temp = format!("Config/.Settings.json.{id}.0.tmp");
        let generated_backup = format!("{JOURNAL_DIR}/backup-{id}-0");
        let cases: Vec<(&str, Vec<Value>)> = vec![
            // The reviewer's probe: the "staged" file is the target itself.
            (
                "temp is the target",
                vec![record_entry(
                    CRASH_S,
                    CRASH_S,
                    Some(&generated_backup),
                    b"old s",
                )],
            ),
            (
                "temp of another id",
                vec![record_entry(
                    CRASH_S,
                    "Config/.Settings.json.8-12.0.tmp",
                    Some(&generated_backup),
                    b"old s",
                )],
            ),
            (
                "temp of another index",
                vec![record_entry(
                    CRASH_S,
                    &format!("Config/.Settings.json.{id}.1.tmp"),
                    Some(&generated_backup),
                    b"old s",
                )],
            ),
            (
                "backup is the target",
                vec![record_entry(
                    CRASH_S,
                    &generated_temp,
                    Some(CRASH_S),
                    b"old s",
                )],
            ),
            (
                "backup of another id",
                vec![record_entry(
                    CRASH_S,
                    &generated_temp,
                    Some(&format!("{JOURNAL_DIR}/backup-8-12-0")),
                    b"old s",
                )],
            ),
            (
                "target inside the journal",
                vec![record_entry(
                    &format!("{JOURNAL_DIR}/lock"),
                    &format!("{JOURNAL_DIR}/.lock.{id}.0.tmp"),
                    None,
                    b"",
                )],
            ),
            (
                "target is another entry's temp",
                vec![
                    record_entry(CRASH_S, &generated_temp, Some(&generated_backup), b"old s"),
                    record_entry(
                        &generated_temp,
                        &format!("Config/..Settings.json.{id}.0.tmp.{id}.1.tmp"),
                        None,
                        b"",
                    ),
                ],
            ),
        ];
        for (case, entries) in cases {
            let root = seeded_crash_root("aliased_record");
            let journal = root.join(JOURNAL_DIR);
            fs::create_dir_all(&journal).expect("journal");
            fs::write(root.join(&generated_backup), "old s").expect("valid backup");
            fs::write(root.join(&generated_temp), "new").expect("staged file");
            let record_path = journal.join(format!("txn-{id}.json"));
            fs::write(
                &record_path,
                json!({"state": "prepared", "files": entries}).to_string(),
            )
            .expect("write record");
            let before = file_state(&root);
            let record = only_record(&root);
            assert!(!record.detail().is_empty(), "{case}: accepted");
            assert!(!record.is_untouched(), "{case}: classified untouched");
            for action in [Recovery::Complete, Recovery::Rollback] {
                assert!(record.check(action).is_err(), "{case}: {action} allowed");
                assert!(
                    only_record(&root).recover(action).is_err(),
                    "{case}: {action} ran"
                );
            }
            assert_eq!(file_state(&root), before, "{case}: files changed");
            assert!(record_path.is_file(), "{case}: record removed");
            fs::remove_dir_all(root).expect("clean root");
        }
    }

    #[cfg(unix)]
    #[test]
    fn recovery_never_follows_a_symlinked_directory_out_of_the_root() {
        let root = seeded_crash_root("symlinked_ancestor");
        let outside = root.with_extension("outside");
        fs::create_dir_all(&outside).expect("outside dir");
        fs::write(outside.join("X.json"), "outside old").expect("outside file");
        std::os::unix::fs::symlink(&outside, root.join("linked")).expect("symlink");
        crash(CrashAfter::Record, || {
            prepare_transaction(
                &root,
                &[FileWrite {
                    relative_path: "linked/X.json",
                    contents: "replaced through a link",
                }],
            )
            .and_then(PreparedTransaction::commit)
        });
        let record = only_record(&root);
        for action in [Recovery::Complete, Recovery::Rollback] {
            let reason = record.check(action).expect_err("a linked path is refused");
            assert!(reason.contains("symbolic link"), "{reason}");
        }
        assert!(!record.is_untouched());
        assert!(only_record(&root).recover(Recovery::Complete).is_err());
        assert_eq!(
            fs::read_to_string(outside.join("X.json")).expect("read outside"),
            "outside old"
        );
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        fs::remove_dir_all(&outside).expect("clean outside");
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn a_stalled_single_file_save_cannot_overwrite_a_resolved_transaction() {
        use std::sync::mpsc;
        use std::time::Duration;
        let root = seeded_crash_root("stalled_single_save");
        let (paused_tx, paused_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel::<()>();
        let saver_root = root.clone();
        // The single-file save passes its unresolved-record check and then
        // stalls right before its rename.
        let saver = std::thread::spawn(move || {
            pause_before_replace(Box::new(move || {
                let _ = paused_tx.send(());
                let _ = resume_rx.recv();
            }));
            write_atomic(&saver_root, CRASH_A, "stale single save")
        });
        paused_rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the save reached its rename");
        // Meanwhile a transaction on the same file is interrupted after its
        // first rename and then resolved by an explicit recovery.
        let (done_tx, done_rx) = mpsc::channel();
        let transaction_root = root.clone();
        let transaction = std::thread::spawn(move || {
            crash(CrashAfter::Rename(1), || {
                prepare_transaction(&transaction_root, &crash_writes(NEW))
                    .and_then(PreparedTransaction::commit)
            });
            let outcome = only_record(&transaction_root).recover(Recovery::Complete);
            let _ = done_tx.send(());
            outcome.map(|_| ())
        });
        let resolved_while_stalled = done_rx.recv_timeout(Duration::from_millis(500)).is_ok();
        resume_tx.send(()).expect("resume the save");
        saver
            .join()
            .expect("saver thread")
            .expect("the single-file save succeeds");
        transaction
            .join()
            .expect("transaction thread")
            .expect("the recovery succeeds");
        assert!(
            !resolved_while_stalled,
            "a transaction and its recovery ran between a single-file save's check and its rename"
        );
        assert_revision(&root, NEW, "the resolved change is not overwritten");
        assert!(interrupted_transactions(&root).expect("list").is_empty());
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn discard_if_untouched_decides_on_the_state_under_the_lock() {
        let root = seeded_crash_root("stale_untouched");
        crash_transaction(&root, CrashAfter::Record);
        let record = only_record(&root);
        assert!(record.is_untouched(), "classified untouched");
        // Between the classification and the discard, an operator's
        // `--complete` replaces the first file and is killed.
        let first = &record.files[0];
        fs::rename(root.join(&first.temp), root.join(&first.relative)).expect("first rename");
        let before = file_state(&root);
        assert_eq!(
            record.discard_if_untouched().expect("no error"),
            None,
            "a change that is no longer untouched is left for the operator"
        );
        assert_eq!(file_state(&root), before, "nothing was rolled back");
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        recover_copy(&root, Recovery::Complete, NEW);
        recover_copy(&root, Recovery::Rollback, OLD);
        fs::remove_dir_all(&root).expect("clean root");

        // Still untouched under the lock: discarded, old revision kept.
        let root = seeded_crash_root("still_untouched");
        crash_transaction(&root, CrashAfter::Record);
        let outcome = only_record(&root)
            .discard_if_untouched()
            .expect("no error")
            .expect("discarded");
        assert_eq!(outcome.action, Recovery::Rollback);
        assert_revision(&root, OLD, "nothing was applied");
        assert_saves_work_again(&root);
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn a_failed_directory_sync_keeps_the_record_on_commit_and_recovery() {
        let root = seeded_crash_root("failed_sync_commit");
        let prepared = prepare_transaction(&root, &crash_writes(NEW)).expect("prepare");
        fail_directory_sync(true);
        let error = prepared.commit();
        fail_directory_sync(false);
        assert!(
            matches!(error, Err(ConfigError::CommitInterrupted { .. })),
            "an undurable commit must not report success: {error:?}"
        );
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        recover_copy(&root, Recovery::Complete, NEW);
        fs::remove_dir_all(&root).expect("clean root");

        for action in [Recovery::Complete, Recovery::Rollback] {
            let root = seeded_crash_root(&format!("failed_sync_{action}"));
            crash_transaction(&root, CrashAfter::Rename(1));
            fail_directory_sync(true);
            let error = only_record(&root).recover(action);
            assert!(
                error.is_err(),
                "{action} reported success without a durable step"
            );
            // Retries while syncing still fails: each one may advance a file,
            // and the one that finds every file done must still not drop the
            // record over undurable renames.
            for retry in 1..=4 {
                let error = only_record(&root).recover(action);
                assert!(
                    error.is_err(),
                    "{action} retry {retry} removed the record before the targets were durable"
                );
            }
            fail_directory_sync(false);
            assert_eq!(
                interrupted_transactions(&root).expect("list").len(),
                1,
                "{action}: the record stays"
            );
            let expected = if action == Recovery::Complete {
                NEW
            } else {
                OLD
            };
            recover_copy(&root, action, expected);
            fs::remove_dir_all(root).expect("clean root");
        }

        // A commit killed after its last rename never synced the target
        // directories: completing must sync them before dropping the record.
        let root = seeded_crash_root("failed_sync_unsynced_commit");
        crash_transaction(&root, CrashAfter::Rename(3));
        fail_directory_sync(true);
        let error = only_record(&root).recover(Recovery::Complete);
        fail_directory_sync(false);
        assert!(
            error.is_err(),
            "complete dropped the record over renames that were never synced"
        );
        assert_eq!(interrupted_transactions(&root).expect("list").len(), 1);
        recover_copy(&root, Recovery::Complete, NEW);
        fs::remove_dir_all(&root).expect("clean root");

        let root = seeded_crash_root("failed_sync_single");
        fail_directory_sync(true);
        let error = write_atomic(&root, CRASH_S, "new s");
        fail_directory_sync(false);
        assert!(
            error.is_err(),
            "a single-file save reports an undurable rename"
        );
        fs::remove_dir_all(root).expect("clean root");
    }

    const LOCK_HOLDER_ROOT: &str = "D4LC_TEST_JOURNAL_LOCK_HOLDER_ROOT";

    /// Child process of [`a_killed_lock_holder_never_blocks_saves`]: takes
    /// the journal lock and waits to be killed.
    #[test]
    #[ignore = "helper process; runs only when spawned by a_killed_lock_holder_never_blocks_saves"]
    fn journal_lock_holder() {
        let Some(root) = env::var_os(LOCK_HOLDER_ROOT) else {
            return;
        };
        let root = PathBuf::from(root);
        let _lock = lock_journal(&root).expect("take the journal lock");
        fs::write(root.join("holder-ready"), "").expect("signal readiness");
        std::thread::sleep(std::time::Duration::from_secs(120));
    }

    #[test]
    fn a_killed_lock_holder_never_blocks_saves() {
        use std::time::{Duration, Instant};
        let root = seeded_crash_root("killed_lock_holder");
        let mut child = std::process::Command::new(env::current_exe().expect("test binary"))
            .args([
                "--exact",
                "persistence::tests::journal_lock_holder",
                "--ignored",
                "--test-threads=1",
            ])
            .env(LOCK_HOLDER_ROOT, &root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the lock holder");
        let deadline = Instant::now() + Duration::from_secs(30);
        while !root.join("holder-ready").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        let probe = open_journal_lock(&root.join(JOURNAL_DIR)).expect("open lock");
        let held_by_child = matches!(probe.try_lock(), Err(fs::TryLockError::WouldBlock));
        drop(probe);
        child.kill().expect("SIGKILL the holder");
        child.wait().expect("reap the holder");
        assert!(held_by_child, "the child held the journal lock");
        assert!(
            root.join(JOURNAL_DIR).join(JOURNAL_LOCK).is_file(),
            "the lock file outlives its holder"
        );
        // The kernel released the dead holder's flock: a save goes through.
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let save_root = root.clone();
        std::thread::spawn(move || {
            let _ = done_tx.send(write_atomic(&save_root, CRASH_S, "after the kill").is_ok());
        });
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(10)), Ok(true));
        fs::remove_dir_all(root).expect("clean root");
    }

    #[test]
    fn a_target_named_like_another_entrys_restore_scratch_is_unreadable() {
        // Reviewer probe: target 2 is exactly target 1's `.restore` scratch
        // path. Target 1 is replaced, target 2 still staged.
        let id = "7-11";
        let first_temp = staged_relative(CRASH_S, id, 0);
        let second = restore_scratch_relative(&first_temp);
        let second_temp = staged_relative(&second, id, 1);
        let root = seeded_crash_root("restore_scratch_alias");
        let journal = root.join(JOURNAL_DIR);
        fs::create_dir_all(&journal).expect("journal");
        fs::write(root.join(CRASH_S), "new").expect("target 1 replaced");
        fs::write(root.join(&second), "old 2").expect("target 2 previous");
        fs::write(root.join(&second_temp), "new").expect("target 2 staged");
        fs::write(root.join(backup_relative(id, 0)), "old s").expect("backup 1");
        fs::write(root.join(backup_relative(id, 1)), "old 2").expect("backup 2");
        let entry = |relative: &str, temp: &str, index: usize, previous: &[u8]| {
            json!({
                "relative": relative, "temp": temp,
                "backup": backup_relative(id, index), "existed_before": true,
                "digest": digest_of_bytes(b"new"), "backup_digest": digest_of_bytes(previous),
            })
        };
        let record_path = journal.join(format!("txn-{id}.json"));
        fs::write(
            &record_path,
            json!({"state": "prepared", "files": [
                entry(CRASH_S, &first_temp, 0, b"old s"),
                entry(&second, &second_temp, 1, b"old 2"),
            ]})
            .to_string(),
        )
        .expect("record");
        let snapshot = |root: &Path| {
            [
                CRASH_S.to_string(),
                second.clone(),
                second_temp.clone(),
                backup_relative(id, 0),
                backup_relative(id, 1),
            ]
            .map(|relative| fs::read_to_string(root.join(relative)).ok())
        };
        let before = snapshot(&root);
        let record = only_record(&root);
        assert!(
            !record.detail().is_empty(),
            "the aliasing record was accepted"
        );
        for action in [Recovery::Complete, Recovery::Rollback] {
            assert!(record.check(action).is_err(), "{action} allowed");
            assert!(only_record(&root).recover(action).is_err(), "{action} ran");
        }
        assert_eq!(snapshot(&root), before, "a refused record changes nothing");
        assert!(record_path.is_file(), "the record is never removed");
        fs::remove_dir_all(root).expect("clean root");
    }
}
