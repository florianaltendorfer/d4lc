//! Single-instance guard for the desktop app.
//!
//! ## Upstream evidence
//!
//! `App.xaml.cs:56-72` — `OnStartup` takes a named `Mutex("D4Companion")`;
//! when the mutex is already held the second launch calls
//! `Application.Current.Shutdown()` and exits immediately.
//!
//! ## Expected behaviour (parity target)
//!
//! - The guard is an advisory `flock` on a lock file under
//!   `$XDG_RUNTIME_DIR/d4lc/` (fallback without `XDG_RUNTIME_DIR`:
//!   `$TMPDIR/d4lc-{uid}/`, created `0700` and refused unless it is a
//!   real directory owned by this user). The lock is kernel-managed: it is released
//!   on every exit path (clean exit or crash) and a leftover lock file
//!   from a crashed holder never wedges the next launch.
//! - The holder writes its pid into the lock file so a second launch can
//!   name the running instance.
//! - A second launch detects the held lock and asks the running instance
//!   to show/restore its window over a unix-domain socket next to the
//!   lock file (the running instance routes the request to the same
//!   `tray::handle_tray_open` show path the tray uses). It then exits 0
//!   like upstream — no silent double instance.
//! - When the show request cannot be delivered (no reachable socket),
//!   the second launch still exits 0 with a structured message naming
//!   the reason.
//! - If the guard itself cannot be established (lock file I/O failure),
//!   the process refuses to start: running unguarded would allow the
//!   silent double instance the guard exists to prevent.
//!
//! - A software-renderer relaunch (`ui_renderer`) replaces the process
//!   through `exec` without releasing the lock: the relaunched process
//!   inherits the locked open file description on its standard input
//!   (marked by [`HANDOVER_ENV`]) and adopts it after checking it is the
//!   lock file. No second launch can take the lock in between. A handover
//!   that does not check out is named and falls back to a normal acquire.
//!
//! The lock file is intentionally never unlinked. `flock` ownership is
//! bound to the open file description, and unlinking on exit races a
//! concurrent launcher into a split-brain double instance (the loser
//! would flock an orphaned inode while the winner creates a fresh file).
//! A leftover file is harmless because the kernel has already dropped
//! the lock.

use std::fs::{self, File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Directory (below the runtime dir) holding the guard files.
const RUNTIME_DIR_NAME: &str = "d4lc";
const LOCK_FILE_NAME: &str = "single-instance.lock";
const SHOW_SOCKET_NAME: &str = "single-instance.sock";
/// Hard ceiling for the one-byte show request so a wedged holder can
/// never hang a second launch.
const SHOW_WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// The server reads one request at a time; a client that connects and
/// stays silent holds it for at most this long.
const SHOW_READ_TIMEOUT: Duration = Duration::from_secs(1);

/// Set on a relaunched process whose standard input is the held lock file
/// (see [`InstanceLock::into_handover`]).
pub const HANDOVER_ENV: &str = "D4LC_INSTANCE_LOCK_ON_STDIN";

/// Outcome of trying to become the single running instance.
#[derive(Debug)]
pub enum AcquireOutcome {
    /// This process now holds the single-instance lock.
    Acquired(InstanceLock),
    /// Another live instance holds the lock; this launch must exit.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the fields are for diagnostics and tests; the guard already printed them"
        )
    )]
    AlreadyRunning {
        /// Pid recorded in the lock file by the holder, when readable.
        holder_pid: Option<u32>,
        /// Whether the show/restore request reached the running instance.
        show_requested: bool,
    },
    /// The guard could not be established (lock file I/O failure). The
    /// caller must not start unguarded — that is exactly the silent
    /// double instance this module prevents.
    Failed,
}

/// The acquired single-instance lock.
///
/// Dropping it releases the advisory `flock` (kernel semantics — also
/// on crash and other non-clean exits) and removes the show-request
/// socket entry. The lock *file* is deliberately kept on disk; see the
/// module docs for the unlink-race rationale.
#[derive(Debug)]
pub struct InstanceLock {
    /// Kept for the process lifetime: the `flock` dies with this file.
    #[allow(dead_code)] // the open description itself holds the lock
    lock_file: File,
    socket_path: PathBuf,
    /// Bound listener, still here until `start_show_request_server`
    /// moves it into the background thread.
    listener: Option<UnixListener>,
}

impl InstanceLock {
    /// Start the background server that turns show requests from second
    /// launches into `show()` calls. One request per connection; the
    /// connection count equals the request count. Without a bound
    /// listener (bind failed at acquire time) this is a no-op and the
    /// module already logged the degradation.
    pub fn start_show_request_server(&mut self, show: impl Fn() + Send + 'static) {
        let Some(listener) = self.listener.take() else {
            return;
        };
        if let Err(error) = std::thread::Builder::new()
            .name("d4lc-single-instance".to_string())
            .spawn(move || serve_show_requests(listener, show))
        {
            eprintln!(
                "[d4lc] single-instance: could not spawn the show-request server ({error}); \
                 second launches will report this instance without focusing it"
            );
        }
    }
}

impl InstanceLock {
    /// Give up this guard for an `exec` of the same binary without ever
    /// releasing the lock: the returned handle shares the locked open file
    /// description, so the `flock` stays held for as long as the handle
    /// (or the relaunched process's standard input made from it) lives.
    /// The show-request socket entry is removed; the relaunched process
    /// binds its own.
    ///
    /// # Errors
    ///
    /// Returns the duplication error; the guard is released then, and the
    /// relaunch has to acquire the lock anew.
    pub fn into_handover(self) -> std::io::Result<File> {
        self.lock_file.try_clone()
    }
}

impl Drop for InstanceLock {
    fn drop(&mut self) {
        // Remove the socket entry; the bound socket itself dies with the
        // process (or the server thread's listener). The advisory flock
        // dies with `lock_file` / the process — the lock file stays on
        // disk on purpose.
        let _ = fs::remove_file(&self.socket_path);
    }
}

/// Try to become the single running instance.
///
/// Resolves the guard directory from `XDG_RUNTIME_DIR`; when the
/// variable is unset or empty (headless/dev environments) a logged
/// fallback below the shared temporary directory is used.
///
/// A relaunched process marked by [`HANDOVER_ENV`] adopts the lock it
/// inherited on its standard input instead (see [`adopt_in`]).
pub fn acquire() -> AcquireOutcome {
    let dir = match runtime_lock_dir() {
        Ok(dir) => dir,
        Err(reason) => {
            eprintln!("[d4lc] single-instance: {reason}; refusing to start an unguarded instance");
            return AcquireOutcome::Failed;
        }
    };
    if std::env::var_os(HANDOVER_ENV).is_some() {
        match std::io::stdin().as_fd().try_clone_to_owned() {
            Ok(inherited) => return adopt_in(&dir, File::from(inherited)),
            Err(error) => eprintln!(
                "[d4lc] single-instance: the handed-over lock on standard input is unusable \
                 ({error}); acquiring the lock normally"
            ),
        }
    }
    acquire_in(&dir)
}

/// Adopt the lock a relaunching process handed over in `inherited`. It is
/// accepted only when it is the lock file under `dir` and holds (or can
/// take) the lock; otherwise the refusal is named and the normal acquire
/// runs, with its usual second-launch outcome when another instance holds
/// the lock.
fn adopt_in(dir: &Path, inherited: File) -> AcquireOutcome {
    let lock_path = dir.join(LOCK_FILE_NAME);
    match check_handover(&lock_path, &inherited) {
        Ok(()) => {
            eprintln!("[d4lc] single-instance: lock handed over by the relaunching process");
            AcquireOutcome::Acquired(start_first_instance(lock_path, inherited))
        }
        Err(reason) => {
            eprintln!(
                "[d4lc] single-instance: the handed-over lock was refused ({reason}); \
                 acquiring the lock normally"
            );
            acquire_in(dir)
        }
    }
}

fn check_handover(lock_path: &Path, inherited: &File) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt as _;
    let held = inherited
        .metadata()
        .map_err(|error| format!("cannot inspect the inherited handle: {error}"))?;
    let on_disk = fs::metadata(lock_path)
        .map_err(|error| format!("cannot inspect {}: {error}", lock_path.display()))?;
    if (held.dev(), held.ino()) != (on_disk.dev(), on_disk.ino()) {
        return Err(format!(
            "the inherited handle is not {}",
            lock_path.display()
        ));
    }
    // Locking a description that already holds the lock keeps it; any
    // other description contends like a second launch.
    match inherited.try_lock() {
        Ok(()) => Ok(()),
        Err(std::fs::TryLockError::WouldBlock) => {
            Err("another process holds the lock, not the inherited handle".to_string())
        }
        Err(std::fs::TryLockError::Error(error)) => Err(format!("could not lock it: {error}")),
    }
}

fn runtime_lock_dir() -> Result<PathBuf, String> {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => Ok(PathBuf::from(dir).join(RUNTIME_DIR_NAME)),
        _ => {
            // The shared temporary directory is writable by every user: the
            // guard directory there is per user and must be private, or
            // another account could pre-create it and squat or redirect the
            // lock file and the show socket.
            use std::os::unix::fs::MetadataExt as _;
            let uid = fs::metadata("/proc/self")
                .map_err(|error| format!("cannot determine the user id from /proc/self: {error}"))?
                .uid();
            let dir = std::env::temp_dir().join(format!("{RUNTIME_DIR_NAME}-{uid}"));
            eprintln!(
                "[d4lc] single-instance: XDG_RUNTIME_DIR is not set; using the private \
                 fallback {}",
                dir.display()
            );
            ensure_private_dir(&dir, uid)?;
            Ok(dir)
        }
    }
}

/// Create `dir` with mode `0700`, or accept an existing one only when it is
/// a real directory (not a symlink) owned by `uid`; group/other bits on an
/// owned directory are removed. The parent must exist.
fn ensure_private_dir(dir: &Path, uid: u32) -> Result<(), String> {
    use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
    match fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("could not create {}: {error}", dir.display())),
    }
    let metadata = fs::symlink_metadata(dir)
        .map_err(|error| format!("could not inspect {}: {error}", dir.display()))?;
    if !metadata.file_type().is_dir() {
        return Err(format!("{} is not a directory", dir.display()));
    }
    if metadata.uid() != uid {
        return Err(format!(
            "{} is owned by uid {}, not by this user ({uid})",
            dir.display(),
            metadata.uid()
        ));
    }
    if metadata.mode() & 0o077 != 0 {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("could not make {} private: {error}", dir.display()))?;
    }
    Ok(())
}

fn acquire_in(dir: &Path) -> AcquireOutcome {
    let lock_path = dir.join(LOCK_FILE_NAME);
    let file = match fs::create_dir_all(dir).and_then(|()| {
        OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
    }) {
        Ok(file) => file,
        Err(error) => {
            let reason = format!(
                "could not open the lock file {}: {error}",
                lock_path.display()
            );
            eprintln!("[d4lc] single-instance: {reason}; refusing to start an unguarded instance");
            return AcquireOutcome::Failed;
        }
    };
    match file.try_lock() {
        Ok(()) => AcquireOutcome::Acquired(start_first_instance(lock_path, file)),
        // flock is per open file description: a second open in the same
        // process contends exactly like a second process.
        Err(std::fs::TryLockError::WouldBlock) => second_launch(&lock_path),
        Err(std::fs::TryLockError::Error(error)) => {
            let reason = format!("could not lock {}: {error}", lock_path.display());
            eprintln!("[d4lc] single-instance: {reason}; refusing to start an unguarded instance");
            AcquireOutcome::Failed
        }
    }
}

/// First-instance setup: record the holder pid and bind the
/// show-request socket. We hold the lock here, so any leftover socket
/// entry is from a crashed previous holder and safe to remove.
fn start_first_instance(lock_path: PathBuf, lock_file: File) -> InstanceLock {
    // Written at offset 0: a handed-over description carries the previous
    // holder's file offset.
    let _ = lock_file.set_len(0).and_then(|()| {
        std::os::unix::fs::FileExt::write_all_at(
            &lock_file,
            format!("{}\n", std::process::id()).as_bytes(),
            0,
        )
    });

    let socket_path = lock_path.with_file_name(SHOW_SOCKET_NAME);
    let _ = fs::remove_file(&socket_path);
    let listener = match UnixListener::bind(&socket_path) {
        Ok(listener) => Some(listener),
        Err(error) => {
            eprintln!(
                "[d4lc] single-instance: could not bind the show-request socket ({error}); \
                 second launches will report this instance without focusing it"
            );
            None
        }
    };
    eprintln!(
        "[d4lc] single-instance: lock acquired ({})",
        lock_path.display()
    );
    InstanceLock {
        lock_file,
        socket_path,
        listener,
    }
}

/// Second-launch behaviour: name the holder, best-effort deliver the
/// show request, and report for the caller's exit.
fn second_launch(lock_path: &Path) -> AcquireOutcome {
    let holder_pid = read_holder_pid(lock_path);
    let pid = holder_pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let socket_path = lock_path.with_file_name(SHOW_SOCKET_NAME);
    let show_requested = request_show(&socket_path);
    if show_requested {
        eprintln!(
            "[d4lc] single-instance: another instance is running (pid={pid}); \
             window show request delivered; exiting."
        );
    } else {
        eprintln!(
            "[d4lc] single-instance: another instance is running (pid={pid}); \
             window show request could not be delivered (no reachable show socket); \
             exiting."
        );
    }
    AcquireOutcome::AlreadyRunning {
        holder_pid,
        show_requested,
    }
}

/// Deliver the one-byte show request. Reachability is exactly the
/// connect + write: the running instance's socket is the only
/// address for its window.
fn request_show(socket_path: &Path) -> bool {
    let Ok(mut stream) = UnixStream::connect(socket_path) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(SHOW_WRITE_TIMEOUT));
    stream.write_all(&[1]).is_ok()
}

fn read_holder_pid(lock_path: &Path) -> Option<u32> {
    let mut contents = String::new();
    File::open(lock_path)
        .and_then(|mut file| file.read_to_string(&mut contents))
        .ok()?;
    contents.trim().parse().ok()
}

/// Accept loop: each connection carries exactly one show request. The
/// read is bounded by [`SHOW_READ_TIMEOUT`], so a silent client cannot
/// block later requests.
fn serve_show_requests(listener: UnixListener, show: impl Fn() + Send) {
    for stream in listener.incoming() {
        let mut stream = match stream {
            Ok(stream) => stream,
            Err(_) => continue,
        };
        if let Err(error) = stream.set_read_timeout(Some(SHOW_READ_TIMEOUT)) {
            eprintln!("[d4lc] single-instance: show request dropped ({error})");
            continue;
        }
        let mut request = [0u8; 1];
        match stream.read(&mut request) {
            Ok(n) if n > 0 => show(),
            _ => {}
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────
//
// Lock lifecycle decision points: acquire + holder pid record,
// second-launch behaviour with and without a reachable show server,
// and the crash shapes — a released/stale lock never wedges the next
// launch. Each test owns an isolated temp directory; flock contention
// between two opens in this process behaves exactly like two processes
// (flock is per open file description).

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::mpsc;

    static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_tmp_root(label: &str) -> PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "d4lc-single-instance-{label}-{}-{n}",
            std::process::id()
        ))
    }

    fn assert_acquired(outcome: AcquireOutcome, what: &str) -> InstanceLock {
        match outcome {
            AcquireOutcome::Acquired(lock) => lock,
            other => panic!("{what}: expected Acquired, got {other:?}"),
        }
    }

    #[test]
    fn first_launch_acquires_lock_and_records_holder_pid() {
        let dir = unique_tmp_root("acquire");
        let lock = assert_acquired(acquire_in(&dir), "first launch");
        // The holder pid is persisted for the second-launch message.
        assert_eq!(
            read_holder_pid(&dir.join(LOCK_FILE_NAME)),
            Some(std::process::id()),
            "the lock file must name the holding process"
        );
        // The show-request socket is bound (kernel-listening) already.
        assert!(dir.join(SHOW_SOCKET_NAME).exists());
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_launch_names_holder_and_reports_unreachable_socket() {
        let dir = unique_tmp_root("second-launch");
        let _first = assert_acquired(acquire_in(&dir), "first launch");
        // A holder with a dead/unbound socket is-unreachable.
        fs::remove_file(dir.join(SHOW_SOCKET_NAME)).unwrap();
        let second = acquire_in(&dir);
        match second {
            AcquireOutcome::AlreadyRunning {
                holder_pid,
                show_requested,
            } => {
                assert_eq!(
                    holder_pid,
                    Some(std::process::id()),
                    "the second launch must report the holder pid"
                );
                assert!(
                    !show_requested,
                    "a removed show socket must be reported as unreachable"
                );
            }
            other => panic!("second launch: expected AlreadyRunning, got {other:?}"),
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn second_launch_show_request_reaches_the_running_instance() {
        let dir = unique_tmp_root("show-request");
        let mut first = assert_acquired(acquire_in(&dir), "first launch");
        let (show_tx, show_rx) = mpsc::channel();
        first.start_show_request_server(move || {
            let _ = show_tx.send(());
        });
        let second = acquire_in(&dir);
        match second {
            AcquireOutcome::AlreadyRunning {
                holder_pid,
                show_requested,
            } => {
                assert_eq!(holder_pid, Some(std::process::id()));
                assert!(
                    show_requested,
                    "a bound show socket must make the request deliverable"
                );
            }
            other => panic!("second launch: expected AlreadyRunning, got {other:?}"),
        }
        show_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the running instance must receive the show request");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn dropped_lock_is_released_and_stale_socket_entry_cleaned() {
        let dir = unique_tmp_root("release");
        let first = assert_acquired(acquire_in(&dir), "first launch");
        let socket_path = dir.join(SHOW_SOCKET_NAME);
        drop(first);
        // The dropped holder cleaned up its socket entry on exit.
        assert!(
            !socket_path.exists(),
            "the dropped holder must remove its show socket entry"
        );
        // Kernel flock release: the very next launch acquires normally.
        let second = assert_acquired(
            acquire_in(&dir),
            "relaunch after a released lock (exit/crash shape)",
        );
        assert!(
            socket_path.exists(),
            "the relaunched holder binds a fresh show socket"
        );
        drop(second);
        let _ = fs::remove_dir_all(&dir);
    }

    /// Environment of the relaunched-process half of the handover test.
    const HANDOVER_TEST_DIR_ENV: &str = "D4LC_TEST_HANDOVER_DIR";

    /// The software-renderer relaunch keeps the lock across the process
    /// boundary: a child that only inherited the handover on its stdin
    /// holds the lock (a new launch is a second launch) until it exits.
    #[test]
    fn a_handed_over_lock_stays_held_across_the_relaunch() {
        use std::io::BufRead as _;
        let dir = unique_tmp_root("handover");
        let first = assert_acquired(acquire_in(&dir), "first launch");
        let handover = first.into_handover().expect("handover handle");
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "single_instance::tests::handover_child_process",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(HANDOVER_TEST_DIR_ENV, &dir)
            .stdin(handover)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn the relaunched half");
        let mut lines = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
        let adopted = lines
            .by_ref()
            .map_while(Result::ok)
            .any(|line| line.contains("HANDOVER-ADOPTED"));
        assert!(adopted, "the relaunched half must adopt the lock");

        // Only the child holds the lock now (this process gave up every
        // handle): a new launch is a second launch that names the child.
        match acquire_in(&dir) {
            AcquireOutcome::AlreadyRunning { holder_pid, .. } => {
                assert_eq!(holder_pid, Some(child.id()));
            }
            other => panic!("expected AlreadyRunning, got {other:?}"),
        }

        fs::write(dir.join("release"), b"").unwrap();
        lines.for_each(drop);
        assert!(child.wait().unwrap().success());
        let after = assert_acquired(acquire_in(&dir), "launch after the relaunch exited");
        drop(after);
        let _ = fs::remove_dir_all(&dir);
    }

    /// The relaunched half of the test above; a no-op when run on its own.
    #[test]
    #[ignore = "spawned by a_handed_over_lock_stays_held_across_the_relaunch"]
    fn handover_child_process() {
        let Some(dir) = std::env::var_os(HANDOVER_TEST_DIR_ENV).map(PathBuf::from) else {
            return;
        };
        let inherited = File::from(std::io::stdin().as_fd().try_clone_to_owned().unwrap());
        let lock = assert_acquired(adopt_in(&dir, inherited), "adopt the handover");
        println!("HANDOVER-ADOPTED");
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while !dir.join("release").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(lock);
    }

    /// A handle that is not the lock file, or a lock file whose lock is
    /// held elsewhere, is refused and the normal acquire decides.
    #[test]
    fn a_handover_that_is_not_the_held_lock_falls_back_to_a_normal_acquire() {
        let dir = unique_tmp_root("handover-refused");
        fs::create_dir_all(&dir).unwrap();
        let unrelated = dir.join("unrelated");
        fs::write(&unrelated, b"").unwrap();
        let lock = assert_acquired(
            adopt_in(&dir, File::open(&unrelated).unwrap()),
            "an unrelated handle with a free lock",
        );

        // The lock file itself, but a description other than the holder's.
        let foreign = File::open(dir.join(LOCK_FILE_NAME)).unwrap();
        assert!(matches!(
            adopt_in(&dir, foreign),
            AcquireOutcome::AlreadyRunning { .. }
        ));
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_silent_client_does_not_block_later_show_requests() {
        let dir = unique_tmp_root("silent-client");
        let mut first = assert_acquired(acquire_in(&dir), "first launch");
        let (show_tx, show_rx) = mpsc::channel();
        first.start_show_request_server(move || {
            let _ = show_tx.send(());
        });
        // Connects and never writes; kept open for the whole test.
        let _silent = UnixStream::connect(dir.join(SHOW_SOCKET_NAME)).unwrap();
        assert!(matches!(
            acquire_in(&dir),
            AcquireOutcome::AlreadyRunning {
                show_requested: true,
                ..
            }
        ));
        show_rx
            .recv_timeout(SHOW_READ_TIMEOUT * 4)
            .expect("the request after a silent client must still be served");
        drop(first);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_fallback_dir_is_private_and_owned_by_this_user() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let root = unique_tmp_root("private-dir");
        fs::create_dir_all(&root).unwrap();
        let uid = fs::metadata("/proc/self").unwrap().uid();
        let mode = |path: &Path| fs::symlink_metadata(path).unwrap().mode() & 0o777;

        // Created fresh with 0700.
        let fresh = root.join("fresh");
        ensure_private_dir(&fresh, uid).unwrap();
        assert_eq!(mode(&fresh), 0o700);
        ensure_private_dir(&fresh, uid).expect("an existing private dir is reused");

        // An own directory left with default permissions is tightened.
        let loose = root.join("loose");
        fs::create_dir(&loose).unwrap();
        fs::set_permissions(&loose, fs::Permissions::from_mode(0o755)).unwrap();
        ensure_private_dir(&loose, uid).unwrap();
        assert_eq!(mode(&loose), 0o700);

        // Another user's directory, a symlink and a plain file are refused
        // and left untouched.
        let foreign = ensure_private_dir(&fresh, uid.wrapping_add(1)).unwrap_err();
        assert!(foreign.contains("is owned by uid"), "{foreign}");
        let link = root.join("link");
        std::os::unix::fs::symlink(&fresh, &link).unwrap();
        assert!(ensure_private_dir(&link, uid)
            .unwrap_err()
            .contains("not a directory"));
        let file = root.join("file");
        fs::write(&file, b"").unwrap();
        assert!(ensure_private_dir(&file, uid).is_err());
        assert_eq!(mode(&file) & 0o700, 0o600, "the file keeps its mode");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_lock_file_from_crashed_holder_does_not_wedge_next_launch() {
        let dir = unique_tmp_root("stale-lock");
        fs::create_dir_all(&dir).unwrap();
        // Crash leftovers: a lock file with a dead holder's pid and an
        // orphaned socket entry, both without a live flock.
        fs::write(dir.join(LOCK_FILE_NAME), b"12345\n").unwrap();
        fs::write(dir.join(SHOW_SOCKET_NAME), b"stale").unwrap();
        let lock = assert_acquired(
            acquire_in(&dir),
            "a stale lock file must never wedge the next launch",
        );
        assert_eq!(
            read_holder_pid(&dir.join(LOCK_FILE_NAME)),
            Some(std::process::id()),
            "the new holder must overwrite the stale pid record"
        );
        drop(lock);
        let _ = fs::remove_dir_all(&dir);
    }
}
