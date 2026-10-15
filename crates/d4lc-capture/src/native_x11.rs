//! Xlib / MIT-SHM capture and pointer FFI.
//!
//! The only module of this crate allowed to use `unsafe`. Invariants every
//! block relies on:
//! - Every Xlib call runs under [`enter_native`] (one process-wide reentrant
//!   lock), after `XInitThreads`.
//! - A `*mut Display` comes from [`open_display`], is non-null, and is
//!   closed exactly once by its single owner (a scope, [`XShmSession`] or
//!   [`PointerDisplay`]).
//! - An `*mut XImage` is non-null, owned by one scope or session, and
//!   destroyed exactly once.

#![allow(unsafe_code)]

use super::{
    validate_rect, zpixmap_to_bgra, CaptureFrame, CaptureMethod, Point, Rect, ZPixmapLayout,
    XWD_Z_PIXMAP,
};
use std::cell::RefCell;
use std::env;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint, c_ulong};
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

#[repr(C)]
struct Display {
    _private: [u8; 0],
}

#[repr(C)]
struct Visual {
    _private: [u8; 0],
}

#[repr(C)]
struct XErrorEvent {
    _private: [u8; 0],
}

#[repr(C)]
struct XShmSegmentInfo {
    shmseg: c_ulong,
    shmid: c_int,
    shmaddr: *mut c_char,
    read_only: c_int,
}

type XErrorHandlerFn = unsafe extern "C" fn(*mut Display, *mut XErrorEvent) -> c_int;
type XErrorHandler = Option<XErrorHandlerFn>;

/// Set by [`record_x_error`] for an error on [`TRAPPED_DISPLAY`].
static X11_ERROR_OCCURRED: AtomicBool = AtomicBool::new(false);
/// Connection whose errors the active [`with_x_error_trap`] collects; null
/// outside a trap.
static TRAPPED_DISPLAY: AtomicPtr<Display> = AtomicPtr::new(ptr::null_mut());
/// Handler that was installed when the active trap started; errors on other
/// connections (the UI toolkit's own Xlib connection) are forwarded to it.
static PREVIOUS_ERROR_HANDLER: std::sync::Mutex<XErrorHandler> = std::sync::Mutex::new(None);
static XLIB_THREADS: std::sync::Once = std::sync::Once::new();
// One process-wide lock for every Xlib call this module makes.
// Xlib keeps global state beyond the per-display handle (the error
// handler, and libX11/libxcb internals that corrupt the allocator
// under concurrent use even with XInitThreads — observed as
// `malloc(): unaligned tcache chunk` with four threads on one Xvfb).
// Reentrant because session teardown (XShmSession::drop) runs inside
// locked capture paths. The live loop makes one frame capture and one
// pointer poll per tick, so the lock is held briefly and rarely contended.
static XLIB_CALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
thread_local! {
    static XLIB_LOCK_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Reentrant guard for [`XLIB_CALL_LOCK`]: the outermost `enter_native`
/// on a thread takes the mutex; nested calls (session teardown inside a
/// locked capture) only bump the depth counter.
struct XlibGuard {
    _inner: Option<std::sync::MutexGuard<'static, ()>>,
}

impl Drop for XlibGuard {
    fn drop(&mut self) {
        XLIB_LOCK_DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

/// Serialize this thread's Xlib calls against every other thread.
fn enter_native() -> XlibGuard {
    ensure_xlib_thread_support();
    let inner = XLIB_LOCK_DEPTH
        .with(|depth| {
            let outermost = depth.get() == 0;
            depth.set(depth.get() + 1);
            outermost
        })
        .then(|| {
            // A poisoned lock means some thread panicked inside Xlib
            // code; the mutex itself is fine, so keep capturing.
            XLIB_CALL_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
        });
    XlibGuard { _inner: inner }
}

/// Open a display connection. The caller must hold an [`XlibGuard`]
/// (directly or through a public entry point) and close the result
/// exactly once through [`close_display`].
fn open_display(name: *const c_char) -> Result<*mut Display, String> {
    ensure_xlib_thread_support();
    // SAFETY: `name` is null or a NUL-terminated string the caller keeps
    // alive for this call; Xlib copies it. Xlib runs under the caller's
    // XlibGuard after XInitThreads.
    let display = unsafe { XOpenDisplay(name) };
    if display.is_null() {
        return Err("XOpenDisplay failed; DISPLAY is unset or X11 is unavailable".to_string());
    }
    Ok(display)
}

/// Close a display opened by [`open_display`]. The caller must hold an
/// [`XlibGuard`].
fn close_display(display: *mut Display) {
    // SAFETY: `display` came from open_display and its single owner calls
    // this exactly once, as its last use of the handle.
    unsafe { XCloseDisplay(display) };
}

/// Xlib without `XInitThreads` corrupts its internal state when two
/// threads use it at once (the live loop captures while the cursor
/// reader polls). It must run before the first Xlib call in the process;
/// every entry point of this module calls it first.
fn ensure_xlib_thread_support() {
    XLIB_THREADS.call_once(|| {
        // SAFETY: XInitThreads takes no arguments and only initializes
        // Xlib's global locks; Once runs it at most once.
        if unsafe { XInitThreads() } == 0 {
            // XInitThreads(3): zero means this libX11 has no thread support
            // or could not initialize it (repeated or late calls return
            // nonzero). This module still serializes its own Xlib calls
            // under XLIB_CALL_LOCK; other Xlib users in the process (winit)
            // then run without libX11's internal locks. Report it once.
            eprintln!(
                "d4lc-capture: XInitThreads returned 0 (libX11 without thread support); \
                 d4lc's own X11 calls stay serialized, other X11 users in this process are \
                 not protected"
            );
        }
    });
}

/// Leading fields of Xlib's `XImage` (X11/Xlib.h), through `blue_mask`.
/// The trailing `obdata` pointer and function table are omitted: images are
/// only read through pointers to Xlib-allocated structs (or, in tests, through
/// this exact prefix), never allocated by Rust for Xlib.
#[repr(C)]
struct XImage {
    width: c_int,
    height: c_int,
    xoffset: c_int,
    format: c_int,
    data: *mut c_char,
    byte_order: c_int,
    bitmap_unit: c_int,
    bitmap_bit_order: c_int,
    bitmap_pad: c_int,
    depth: c_int,
    bytes_per_line: c_int,
    bits_per_pixel: c_int,
    red_mask: c_ulong,
    green_mask: c_ulong,
    blue_mask: c_ulong,
}

#[link(name = "X11")]
unsafe extern "C" {
    fn XOpenDisplay(display_name: *const c_char) -> *mut Display;
    fn XCloseDisplay(display: *mut Display) -> c_int;
    fn XDefaultRootWindow(display: *mut Display) -> c_ulong;
    fn XDefaultScreen(display: *mut Display) -> c_int;
    fn XDefaultVisual(display: *mut Display, screen_number: c_int) -> *mut Visual;
    fn XDefaultDepth(display: *mut Display, screen_number: c_int) -> c_int;
    fn XSync(display: *mut Display, discard: c_int) -> c_int;
    fn XSetErrorHandler(handler: XErrorHandler) -> XErrorHandler;
    fn XGetImage(
        display: *mut Display,
        drawable: c_ulong,
        x: c_int,
        y: c_int,
        width: c_uint,
        height: c_uint,
        plane_mask: c_ulong,
        format: c_int,
    ) -> *mut XImage;
    fn XDestroyImage(image: *mut XImage) -> c_int;
    fn XQueryPointer(
        display: *mut Display,
        window: c_ulong,
        root_return: *mut c_ulong,
        child_return: *mut c_ulong,
        root_x_return: *mut c_int,
        root_y_return: *mut c_int,
        win_x_return: *mut c_int,
        win_y_return: *mut c_int,
        mask_return: *mut c_uint,
    ) -> c_int;
    fn XInitThreads() -> c_int;
}

#[link(name = "Xext")]
unsafe extern "C" {
    fn XShmQueryExtension(display: *mut Display) -> c_int;
    fn XShmCreateImage(
        display: *mut Display,
        visual: *mut Visual,
        depth: c_uint,
        format: c_int,
        data: *mut c_char,
        shminfo: *mut XShmSegmentInfo,
        width: c_uint,
        height: c_uint,
    ) -> *mut XImage;
    fn XShmAttach(display: *mut Display, shminfo: *mut XShmSegmentInfo) -> c_int;
    fn XShmDetach(display: *mut Display, shminfo: *mut XShmSegmentInfo) -> c_int;
    fn XShmGetImage(
        display: *mut Display,
        drawable: c_ulong,
        image: *mut XImage,
        x: c_int,
        y: c_int,
        plane_mask: c_ulong,
    ) -> c_int;
}

unsafe extern "C" {
    fn shmget(key: c_int, size: usize, shmflg: c_int) -> c_int;
    fn shmat(shmid: c_int, shmaddr: *const c_void, shmflg: c_int) -> *mut c_void;
    fn shmdt(shmaddr: *const c_void) -> c_int;
    fn shmctl(shmid: c_int, cmd: c_int, buf: *mut c_void) -> c_int;
}

const IPC_PRIVATE: c_int = 0;
const IPC_CREAT: c_int = 0o1000;
const IPC_RMID: c_int = 0;
const SHM_PERMISSIONS: c_int = 0o600;
const SHM_RDONLY_FALSE: c_int = 0;

pub(super) struct NativeCapturedFrame {
    pub frame: CaptureFrame,
    pub method: CaptureMethod,
    pub fallback_reason: Option<String>,
}

pub(super) fn capture_drawable(
    drawable: u64,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<NativeCapturedFrame, String> {
    let _xlib = enter_native();
    if width == 0 || height == 0 {
        return Err("native X11 capture dimensions must be non-zero".to_string());
    }
    let drawable = c_ulong::try_from(drawable)
        .map_err(|error| format!("X11 drawable id does not fit platform c_ulong: {error}"))?;
    if xshm_disabled_by_env() {
        return with_display(|display| {
            capture_drawable_with_xgetimage(display, drawable, x, y, width, height, None)
        });
    }
    // Persistent MIT-SHM session: the display, XShm image and attached
    // segment stay alive across frames. A session that cannot serve the
    // frame is dropped and this frame falls back to the fresh path below,
    // which attaches a new segment or reports the XGetImage fallback.
    if let SessionOutcome::Frame(result) = reuse_xshm_session(drawable, x, y, width, height) {
        return result;
    }
    capture_with_fresh_session(drawable, x, y, width, height)
}

pub(super) fn capture_root_region(region: Rect) -> Result<NativeCapturedFrame, String> {
    let _xlib = enter_native();
    validate_rect(region)?;
    if region.x < 0 || region.y < 0 {
        return Err("manual region cannot start outside the root window".to_string());
    }
    if xshm_disabled_by_env() {
        return with_display(|display| {
            // SAFETY: `display` is the live connection with_display opened
            // and owns for this closure; the XlibGuard above is held.
            let root = unsafe { XDefaultRootWindow(display) };
            capture_drawable_with_xgetimage(
                display,
                root,
                region.x,
                region.y,
                region.width,
                region.height,
                None,
            )
        });
    }
    // SAFETY: as above — live display owned by with_display, lock held.
    let root = with_display(|display| Ok(unsafe { XDefaultRootWindow(display) }))?;
    if let SessionOutcome::Frame(result) =
        reuse_xshm_session(root, region.x, region.y, region.width, region.height)
    {
        return result;
    }
    capture_with_fresh_session(root, region.x, region.y, region.width, region.height)
}

/// Fresh XShm attach for one capture. On success the new session takes
/// ownership of its display connection, so this function only closes the
/// display when no session owns it.
fn capture_with_fresh_session(
    drawable: c_ulong,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<NativeCapturedFrame, String> {
    capture_with_fresh_session_on(None, drawable, x, y, width, height)
}

/// [`capture_with_fresh_session`] on `display_name`, or on `DISPLAY`
/// when it is `None`.
pub(super) fn capture_with_fresh_session_on(
    display_name: Option<&str>,
    drawable: c_ulong,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<NativeCapturedFrame, String> {
    let _xlib = enter_native();
    let name = display_name
        .map(std::ffi::CString::new)
        .transpose()
        .map_err(|error| format!("invalid display name: {error}"))?;
    let display = open_display(name.as_ref().map_or(ptr::null(), |n| n.as_ptr()))?;
    let outcome = capture_drawable_with_xshm(display, drawable, x, y, width, height);
    let (result, session_owns_display) = match outcome {
        Ok((frame, Some(session))) => {
            store_xshm_session(session);
            (Ok(frame), true)
        }
        Ok((frame, None)) => (Ok(frame), false),
        Err(xshm_error) => {
            let result = capture_drawable_with_xgetimage(
                display,
                drawable,
                x,
                y,
                width,
                height,
                Some(format!(
                    "xshm unavailable; used XGetImage fallback: {xshm_error}"
                )),
            );
            (result, false)
        }
    };
    if !session_owns_display {
        close_display(display);
    }
    result
}

fn with_display<T>(f: impl FnOnce(*mut Display) -> Result<T, String>) -> Result<T, String> {
    let _xlib = enter_native();
    let display = open_display(ptr::null())?;
    let result = f(display);
    close_display(display);
    result
}

fn capture_drawable_with_xgetimage(
    display: *mut Display,
    drawable: c_ulong,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    fallback_reason: Option<String>,
) -> Result<NativeCapturedFrame, String> {
    // A window that is gone or unmapped makes the server answer with an
    // error; the trap turns it into an `Err` instead of handing it to the
    // process-wide Xlib error handler.
    // SAFETY: `display` is a live connection owned by the caller, which
    // holds the XlibGuard; all other arguments are plain values.
    let image = with_x_error_trap(display, || unsafe {
        XGetImage(
            display,
            drawable,
            x,
            y,
            width,
            height,
            c_ulong::MAX,
            XWD_Z_PIXMAP as c_int,
        )
    });
    let image = match image {
        Ok(image) if !image.is_null() => image,
        Ok(_) => return Err("XGetImage returned null".to_string()),
        // Xlib answers a failed request with a null image, so nothing
        // is left to free here.
        Err(error) => return Err(format!("XGetImage failed: {error}")),
    };
    // SAFETY: `image` is the non-null XGetImage result owned by this
    // function. libX11 >= 1.6.4 returns a ZPixmap image only when its reply
    // buffer holds at least `bytes_per_line * height` bytes (GetImage.c
    // reply validation), which is the buffer contract of
    // ximage_to_capture_frame. Nothing else references the image.
    let result = unsafe { ximage_to_capture_frame(image) }.map(|frame| NativeCapturedFrame {
        frame,
        method: CaptureMethod::X11NativeXGetImage,
        fallback_reason,
    });
    // SAFETY: XGetImage allocated `image` and its data; this is the only
    // release and `image` is not used afterwards.
    unsafe {
        XDestroyImage(image);
    }
    result
}

fn capture_drawable_with_xshm(
    display: *mut Display,
    drawable: c_ulong,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> Result<(NativeCapturedFrame, Option<XShmSession>), String> {
    // SAFETY (this and the three screen queries below): `display` is a
    // live connection owned by the caller, which holds the XlibGuard;
    // `screen` is the display's own default screen number.
    if unsafe { XShmQueryExtension(display) } == 0 {
        return Err("MIT-SHM extension is not available".to_string());
    }

    let screen = unsafe { XDefaultScreen(display) };
    let visual = unsafe { XDefaultVisual(display, screen) };
    if visual.is_null() {
        return Err("XDefaultVisual returned null".to_string());
    }
    let depth = unsafe { XDefaultDepth(display, screen) };
    let depth =
        c_uint::try_from(depth).map_err(|error| format!("invalid X11 default depth: {error}"))?;

    // Heap-allocate the segment info: XShmCreateImage stores a pointer to
    // it in `image->obdata`, and XShmGetImage reads that pointer on every
    // call. The address must stay valid for the whole session lifetime, so
    // the struct may not live on this function's stack — a moved Box keeps
    // the heap address stable.
    let mut shminfo = Box::new(XShmSegmentInfo {
        shmseg: 0,
        shmid: -1,
        shmaddr: ptr::null_mut(),
        read_only: SHM_RDONLY_FALSE,
    });
    // SAFETY: `display`/`visual` are live (see above). Null `data` makes
    // Xlib allocate only the XImage header; the data pointer is set to the
    // attached segment below. `shminfo` is a heap allocation owned by the
    // session for as long as the image exists, as XShmCreateImage requires.
    let image = unsafe {
        XShmCreateImage(
            display,
            visual,
            depth,
            XWD_Z_PIXMAP as c_int,
            ptr::null_mut(),
            &mut *shminfo,
            width,
            height,
        )
    };
    if image.is_null() {
        return Err("XShmCreateImage returned null".to_string());
    }

    // SAFETY: `image` is non-null, freshly allocated by Xlib and referenced
    // by nothing else until the session takes it over; the borrow ends
    // before any other access to `*image`.
    let image_ref = unsafe { &mut *image };
    let bytes_per_line = usize::try_from(image_ref.bytes_per_line)
        .map_err(|error| format!("invalid XShm bytes_per_line: {error}"));
    let height_usize = usize::try_from(height).map_err(|error| error.to_string());
    let shm_size = bytes_per_line
        .and_then(|stride| height_usize.map(|rows| (stride, rows)))
        .and_then(|(stride, rows)| {
            stride
                .checked_mul(rows)
                .ok_or_else(|| "XShm image size overflow".to_string())
        });
    let shm_size = match shm_size {
        Ok(size) if size > 0 => size,
        // SAFETY (both arms): `image` has null data and no segment yet;
        // this is its only release and it is not used afterwards.
        Ok(_) => {
            unsafe {
                XDestroyImage(image);
            }
            return Err("XShm image size must be non-zero".to_string());
        }
        Err(error) => {
            unsafe {
                XDestroyImage(image);
            }
            return Err(error);
        }
    };

    // SAFETY: plain syscall; a private segment of exactly
    // `bytes_per_line * height` bytes, the buffer XShmGetImage fills.
    let shmid = unsafe { shmget(IPC_PRIVATE, shm_size, IPC_CREAT | SHM_PERMISSIONS) };
    if shmid < 0 {
        // SAFETY: `image` still has null data; only release.
        unsafe {
            XDestroyImage(image);
        }
        return Err("shmget failed".to_string());
    }
    shminfo.shmid = shmid;

    // SAFETY: maps the segment just created; a null address lets the
    // kernel choose. The mapping lives until cleanup_xshm calls shmdt.
    let shmaddr = unsafe { shmat(shmid, ptr::null(), 0) };
    if shmaddr as isize == -1 {
        // SAFETY: `shmid` is our unmapped segment and `image` still has null
        // data; both are released exactly once here.
        unsafe {
            shmctl(shmid, IPC_RMID, ptr::null_mut());
            XDestroyImage(image);
        }
        return Err("shmat failed".to_string());
    }
    shminfo.shmaddr = shmaddr.cast::<c_char>();
    image_ref.data = shminfo.shmaddr;

    let mut attached = false;
    let mut remove_marked = false;
    // SAFETY: live display under the XlibGuard; `shminfo` describes the
    // mapped segment and outlives the attachment (owned by the session).
    let attach_result = with_x_error_trap(display, || unsafe {
        let attached_value = XShmAttach(display, &mut *shminfo);
        XSync(display, 0);
        attached_value
    });
    match attach_result {
        Ok(value) if value != 0 => {
            attached = true;
            // SAFETY: marks our own segment for removal once both this
            // process and the X server have detached; the mapping stays
            // valid until then.
            unsafe {
                shmctl(shmid, IPC_RMID, ptr::null_mut());
            }
            remove_marked = true;
        }
        // SAFETY (both arms): live display under the XlibGuard; `image` and
        // `shminfo` were created above, are released only here and not used
        // afterwards.
        Ok(_) => {
            unsafe { cleanup_xshm(display, image, &mut shminfo, attached, remove_marked) };
            return Err("XShmAttach returned false".to_string());
        }
        Err(error) => {
            unsafe { cleanup_xshm(display, image, &mut shminfo, attached, remove_marked) };
            return Err(format!("XShmAttach failed: {error}"));
        }
    }

    let mut session = XShmSession {
        display,
        drawable,
        width,
        height,
        image,
        shminfo,
        attached,
        remove_marked,
        owns_display: false,
    };
    // On error `?` drops the session, which releases the image and the
    // segment but leaves the display open: the caller still owns it and
    // falls back to XGetImage on it. Only a session that captured takes
    // the display over and is returned to the caller for storage.
    let frame = session.get_image(x, y)?;
    session.owns_display = true;
    Ok((frame, Some(session)))
}

/// Persistent per-thread MIT-SHM capture session.
///
/// Keeps one X11 display connection, one XShm image, and one attached shm
/// segment alive for a (drawable, width, height) key so the per-frame hot
/// path is a single `XShmGetImage` round trip instead of the full
/// open-display/create-image/shmget/shmat/attach/detach/destroy cycle.
/// Dropping the session releases the image and segment through
/// [`cleanup_xshm`] and closes the display when the session owns it.
struct XShmSession {
    display: *mut Display,
    drawable: c_ulong,
    width: u32,
    height: u32,
    image: *mut XImage,
    shminfo: Box<XShmSegmentInfo>,
    attached: bool,
    remove_marked: bool,
    /// Whether dropping the session closes `display`. A session that
    /// failed its first capture leaves the display to the caller.
    owns_display: bool,
}

impl XShmSession {
    fn matches(&self, drawable: c_ulong, width: u32, height: u32) -> bool {
        self.drawable == drawable && self.width == width && self.height == height
    }

    fn get_image(&mut self, x: i32, y: i32) -> Result<NativeCapturedFrame, String> {
        // SAFETY: the session owns the live display, the image and the
        // attached segment; with_x_error_trap holds the XlibGuard. The
        // server bounds its write by the segment size, which XSync awaits.
        let get_result = with_x_error_trap(self.display, || unsafe {
            let value = XShmGetImage(self.display, self.drawable, self.image, x, y, c_ulong::MAX);
            XSync(self.display, 0);
            value
        });
        match get_result {
            // SAFETY: `self.image` is the session's live image; its data is
            // the mapped segment of `bytes_per_line * height` bytes
            // (`shm_size`), fully written once XSync returned. The segment is
            // private to this thread's session, so nothing writes it while
            // the frame is converted.
            Ok(value) if value != 0 => {
                unsafe { ximage_to_capture_frame(self.image) }.map(|frame| NativeCapturedFrame {
                    frame,
                    method: CaptureMethod::X11NativeXShm,
                    fallback_reason: None,
                })
            }
            Ok(_) => Err("XShmGetImage returned false".to_string()),
            Err(error) => Err(format!("XShmGetImage failed: {error}")),
        }
    }
}

impl Drop for XShmSession {
    fn drop(&mut self) {
        let _xlib = enter_native();
        // SAFETY: the session exclusively owns `image` and `shminfo`, and
        // `display` is still open (closed below when owned, otherwise by the
        // caller after this drop); nothing uses them after this call.
        unsafe {
            cleanup_xshm(
                self.display,
                self.image,
                &mut self.shminfo,
                self.attached,
                self.remove_marked,
            );
        }
        if self.owns_display {
            close_display(self.display);
        }
    }
}

thread_local! {
    static XSHM_SESSION: RefCell<Option<XShmSession>> = const { RefCell::new(None) };
    static POINTER_DISPLAY: RefCell<Option<PointerDisplay>> = const { RefCell::new(None) };
}

/// Display connection kept open for per-frame pointer queries.
struct PointerDisplay(*mut Display);

impl Drop for PointerDisplay {
    fn drop(&mut self) {
        let _xlib = enter_native();
        close_display(self.0);
    }
}

/// Root-window pointer position. The connection is opened on first use
/// and reopened after a failed query.
pub(super) fn query_pointer() -> Result<Point, String> {
    let _xlib = enter_native();
    POINTER_DISPLAY.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let display = open_display(ptr::null())?;
            *slot = Some(PointerDisplay(display));
        }
        let display = slot.as_ref().map_or(ptr::null_mut(), |held| held.0);
        let queried = query_root_pointer(display);
        if queried.is_err() {
            slot.take();
        }
        queried
    })
}

fn query_root_pointer(display: *mut Display) -> Result<Point, String> {
    let mut root: c_ulong = 0;
    let mut child: c_ulong = 0;
    let (mut root_x, mut root_y, mut win_x, mut win_y): (c_int, c_int, c_int, c_int) = (0, 0, 0, 0);
    let mut mask: c_uint = 0;
    // SAFETY: `display` is a live connection owned by the caller (the
    // thread's PointerDisplay); every out-pointer is a local that outlives
    // the call.
    with_x_error_trap(display, || unsafe {
        let root_window = XDefaultRootWindow(display);
        // The return value is False only when the pointer is on another
        // screen; root_x/root_y are then relative to that screen's root,
        // which is still the global position.
        XQueryPointer(
            display,
            root_window,
            &mut root,
            &mut child,
            &mut root_x,
            &mut root_y,
            &mut win_x,
            &mut win_y,
            &mut mask,
        );
    })
    .map(|()| Point {
        x: root_x,
        y: root_y,
    })
    .map_err(|error| format!("XQueryPointer failed: {error}"))
}

/// Pointer query on an explicitly named display (tests use a private
/// Xvfb so they never touch the desktop session).
#[cfg(test)]
pub(super) fn query_pointer_on(display_name: &str) -> Result<Point, String> {
    let _xlib = enter_native();
    let name = std::ffi::CString::new(display_name).map_err(|error| error.to_string())?;
    let display = open_display(name.as_ptr())?;
    let held = PointerDisplay(display);
    query_root_pointer(held.0)
}

enum SessionOutcome {
    /// The session served (or definitively failed) this frame.
    Frame(Result<NativeCapturedFrame, String>),
    /// No usable session for this (drawable, size); run the fresh path.
    RetryFresh,
}

fn reuse_xshm_session(
    drawable: c_ulong,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
) -> SessionOutcome {
    XSHM_SESSION.with(|slot| {
        let mut slot = slot.borrow_mut();
        match slot.as_mut() {
            Some(session) if session.matches(drawable, width, height) => {
                match session.get_image(x, y) {
                    Ok(frame) => SessionOutcome::Frame(Ok(frame)),
                    Err(_) => {
                        // Broken session (X error, window gone): drop it and
                        // let the fresh path rebuild it or fall back to
                        // XGetImage.
                        slot.take();
                        SessionOutcome::RetryFresh
                    }
                }
            }
            Some(_) => {
                // Geometry changed for the same slot: rebuild fresh.
                slot.take();
                SessionOutcome::RetryFresh
            }
            None => SessionOutcome::RetryFresh,
        }
    })
}

fn store_xshm_session(session: XShmSession) {
    XSHM_SESSION.with(|slot| {
        // Any previous session was already dropped by the reuse path when it
        // did not match; replacing via Drop is safe either way.
        *slot.borrow_mut() = Some(session);
    });
}

/// Release an XShm image and its segment in the order Xlib requires:
/// detach from the server, unhook the segment from the image, destroy the
/// image header, unmap, and remove the segment unless already marked.
///
/// # Safety
///
/// `display` is the live connection the image was created on and the
/// caller holds the XlibGuard; `image` is a non-null XShmCreateImage result
/// that is not used after this call; `shminfo` is the segment info it was
/// created with (`shmaddr` null or the mapping, `shmid` negative or ours).
unsafe fn cleanup_xshm(
    display: *mut Display,
    image: *mut XImage,
    shminfo: &mut XShmSegmentInfo,
    attached: bool,
    remove_marked: bool,
) {
    // SAFETY: guaranteed by the caller contract above. The image's data
    // pointer is nulled before XDestroyImage so Xlib does not free() the
    // shm mapping.
    unsafe {
        if attached {
            XShmDetach(display, shminfo);
            XSync(display, 0);
        }
        if !shminfo.shmaddr.is_null() {
            (*image).data = ptr::null_mut();
        }
        XDestroyImage(image);
        if !shminfo.shmaddr.is_null() {
            shmdt(shminfo.shmaddr.cast::<c_void>());
        }
        if shminfo.shmid >= 0 && !remove_marked {
            shmctl(shminfo.shmid, IPC_RMID, ptr::null_mut());
        }
    }
}

fn xshm_disabled_by_env() -> bool {
    matches!(
        env::var("D4LC_DISABLE_XSHM").ok().as_deref(),
        Some("1" | "true" | "TRUE" | "yes" | "YES")
    )
}

/// Run `f` (Xlib requests on `display`) and report whether the X server
/// answered any of them with an error. Errors on other connections are
/// forwarded to the handler that was active before the trap; with no
/// application handler that is Xlib's default, which exits the process,
/// exactly as for an error outside a trap.
///
/// `display` must stay open until this returns (also when `f` unwinds).
/// Not nestable: an inner trap would reset the outer trap's error flag.
fn with_x_error_trap<T>(display: *mut Display, f: impl FnOnce() -> T) -> Result<T, String> {
    // XSetErrorHandler and the shared flag are process-global; hold the
    // Xlib lock from before the flag reset until the previous handler is
    // restored, so no other thread's request can slip in between. Declared
    // before `installed`, so it is released after the handler is restored.
    let _xlib = enter_native();
    X11_ERROR_OCCURRED.store(false, Ordering::SeqCst);
    TRAPPED_DISPLAY.store(display, Ordering::SeqCst);
    // SAFETY: XSetErrorHandler only swaps Xlib's global handler pointer;
    // record_x_error has the XErrorHandler signature and cannot unwind.
    let previous = unsafe { XSetErrorHandler(Some(record_x_error)) };
    let installed = ErrorTrapGuard { display, previous };
    // Stored after the swap (never while calling into Xlib, which would risk
    // a lock-order inversion with a handler running on another thread). An
    // error on another connection inside this short gap goes to the handler
    // recorded by the previous trap (the same handler in practice), or is
    // dropped before the first trap has recorded one.
    *PREVIOUS_ERROR_HANDLER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = previous;
    let result = f();
    drop(installed);
    if X11_ERROR_OCCURRED.load(Ordering::SeqCst) {
        Err("X11 error was reported".to_string())
    } else {
        Ok(result)
    }
}

/// Restores the error handler of a [`with_x_error_trap`] and clears the
/// trapped display on every exit path, including unwinding out of `f`.
struct ErrorTrapGuard {
    display: *mut Display,
    previous: XErrorHandler,
}

impl Drop for ErrorTrapGuard {
    fn drop(&mut self) {
        // SAFETY: `display` is live until the trap returns (contract of
        // with_x_error_trap) and the trap's XlibGuard is still held. XSync
        // delivers the errors of the trapped requests before the previous
        // handler is reinstalled.
        unsafe {
            XSync(self.display, 0);
            XSetErrorHandler(self.previous);
        }
        TRAPPED_DISPLAY.store(ptr::null_mut(), Ordering::SeqCst);
    }
}

/// Xlib error handler installed by [`with_x_error_trap`]. Records errors of
/// the trapped connection and forwards errors of any other connection to the
/// handler that was active before the trap. XSetErrorHandler never returns
/// null (it reports Xlib's default handler instead), so the handler is
/// missing only before the first trap has recorded one; it is never
/// `record_x_error` itself unless traps were nested. Both cases drop the
/// error instead of recursing.
unsafe extern "C" fn record_x_error(display: *mut Display, event: *mut XErrorEvent) -> c_int {
    if display == TRAPPED_DISPLAY.load(Ordering::SeqCst) {
        X11_ERROR_OCCURRED.store(true, Ordering::SeqCst);
        return 0;
    }
    let previous = *PREVIOUS_ERROR_HANDLER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    match previous {
        Some(handler) if !std::ptr::fn_addr_eq(handler, record_x_error as XErrorHandlerFn) => {
            // SAFETY: `handler` was installed by other code for exactly this
            // callback; Xlib's own arguments are passed through unchanged.
            unsafe { handler(display, event) }
        }
        _ => 0,
    }
}

/// Convert an `XImage` filled by `XGetImage` or `XShmGetImage` into a BGRA
/// frame.
///
/// # Safety
///
/// `image` is non-null and points to a live `XImage` whose non-null `data`
/// holds at least `bytes_per_line * height` readable bytes that nothing
/// writes during the call.
unsafe fn ximage_to_capture_frame(image: *const XImage) -> Result<CaptureFrame, String> {
    // SAFETY: non-null, aligned and live per the caller contract.
    let image = unsafe { &*image };
    let layout = ximage_layout(image)?;
    if image.data.is_null() {
        return Err("X11 image has null data".to_string());
    }
    // required_len() checked `bytes_per_line >= width * bytes_per_pixel`,
    // so it is at most `bytes_per_line * height`.
    let len = layout.required_len()?;
    // SAFETY: `data` is non-null and holds `bytes_per_line * height >= len`
    // readable bytes that are not written while the slice lives (caller
    // contract); u8 has no alignment or validity requirement.
    let data = unsafe { slice::from_raw_parts(image.data.cast::<u8>(), len) };
    zpixmap_to_bgra(data, &layout)
}

/// Validate the header of an X11 image and describe its pixel layout.
/// Only ZPixmap images without a left pad (`xoffset == 0`) and with a
/// non-zero depth are accepted; masks must fit 32 bits.
fn ximage_layout(image: &XImage) -> Result<ZPixmapLayout, String> {
    if image.format != XWD_Z_PIXMAP as c_int {
        return Err(format!("unsupported X11 image format {}", image.format));
    }
    if image.xoffset != 0 {
        return Err(format!("unsupported X11 image xoffset {}", image.xoffset));
    }
    if image.depth <= 0 {
        return Err(format!("invalid X11 image depth {}", image.depth));
    }
    let dimension = |value: c_int, name: &str| {
        u32::try_from(value)
            .ok()
            .filter(|value| *value > 0)
            .ok_or_else(|| format!("invalid X11 image {name} {value}"))
    };
    let mask = |value: c_ulong, name: &str| {
        u32::try_from(value)
            .map_err(|_| format!("X11 image {name} mask {value:#x} exceeds 32 bits"))
    };
    Ok(ZPixmapLayout {
        width: dimension(image.width, "width")?,
        height: dimension(image.height, "height")?,
        stride: usize::try_from(image.bytes_per_line)
            .map_err(|_| format!("invalid X11 image bytes_per_line {}", image.bytes_per_line))?,
        bits_per_pixel: u32::try_from(image.bits_per_pixel)
            .map_err(|_| format!("invalid X11 image bits_per_pixel {}", image.bits_per_pixel))?,
        byte_order: u32::try_from(image.byte_order)
            .map_err(|_| format!("invalid X11 image byte order {}", image.byte_order))?,
        red_mask: mask(image.red_mask, "red")?,
        green_mask: mask(image.green_mask, "green")?,
        blue_mask: mask(image.blue_mask, "blue")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BYTE_ORDER_LSB_FIRST, BYTE_ORDER_MSB_FIRST};

    /// 32-bpp TrueColor ZPixmap header over `data` (owned by the test).
    fn ximage(width: c_int, height: c_int, bytes_per_line: c_int, data: &mut [u8]) -> XImage {
        XImage {
            width,
            height,
            xoffset: 0,
            format: XWD_Z_PIXMAP as c_int,
            data: data.as_mut_ptr().cast::<c_char>(),
            byte_order: BYTE_ORDER_LSB_FIRST as c_int,
            bitmap_unit: 32,
            bitmap_bit_order: BYTE_ORDER_LSB_FIRST as c_int,
            bitmap_pad: 32,
            depth: 24,
            bytes_per_line,
            bits_per_pixel: 32,
            red_mask: 0x00ff_0000,
            green_mask: 0x0000_ff00,
            blue_mask: 0x0000_00ff,
        }
    }

    #[test]
    fn ximage_with_odd_width_and_padded_stride_converts() {
        // 3x2 at 32 bpp: 12 pixel bytes per row, 16-byte stride (4 pad).
        let mut data = vec![0xEE; 16 * 2];
        for (index, bgr) in [
            [1, 2, 3],
            [4, 5, 6],
            [7, 8, 9],
            [10, 11, 12],
            [13, 14, 15],
            [16, 17, 18],
        ]
        .iter()
        .enumerate()
        {
            let (x, y) = (index % 3, index / 3);
            data[y * 16 + x * 4..][..3].copy_from_slice(bgr);
        }
        let image = ximage(3, 2, 16, &mut data);
        // SAFETY: `image` is live and its data spans bytes_per_line * height
        // = 32 bytes owned by `data`, which nothing else touches.
        let frame = unsafe { ximage_to_capture_frame(&image) }
            .unwrap_or_else(|error| panic!("conversion failed: {error}"));
        assert_eq!((frame.width, frame.height, frame.stride), (3, 2, 12));
        assert_eq!(
            frame.pixels_bgra,
            [
                1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, //
                10, 11, 12, 255, 13, 14, 15, 255, 16, 17, 18, 255,
            ]
        );
    }

    #[test]
    fn ximage_with_stride_shorter_than_a_row_is_rejected_before_reading() {
        // bytes_per_line 4 for a 2-px, 32-bpp row (8 bytes): a size formula
        // without the stride bound builds a slice past the 8-byte allocation.
        let mut data = vec![0u8; 8];
        let image = ximage(2, 2, 4, &mut data);
        // SAFETY: `image` is live; its declared buffer (4 * 2 = 8 bytes) is
        // exactly `data`. The function must reject it without reading.
        let result = unsafe { ximage_to_capture_frame(&image) };
        assert!(result.is_err(), "short stride must be rejected");
    }

    #[test]
    fn ximage_with_null_data_is_rejected() {
        let mut data = vec![0u8; 8];
        let mut image = ximage(2, 1, 8, &mut data);
        image.data = ptr::null_mut();
        // SAFETY: `image` is live; null data is checked before any read.
        assert!(unsafe { ximage_to_capture_frame(&image) }.is_err());
    }

    #[test]
    fn ximage_header_validation_rejects_unsupported_layouts() {
        let mut data = vec![0u8; 64];
        let base = ximage(2, 2, 8, &mut data);
        type Mutation = fn(&mut XImage);
        let cases: [(&str, Mutation); 11] = [
            ("XYPixmap format", |image| image.format = 1),
            ("left pad", |image| image.xoffset = 1),
            ("zero depth", |image| image.depth = 0),
            ("zero width", |image| image.width = 0),
            ("negative height", |image| image.height = -1),
            ("negative stride", |image| image.bytes_per_line = -8),
            ("short stride", |image| image.bytes_per_line = 4),
            ("8 bpp", |image| image.bits_per_pixel = 8),
            ("negative bpp", |image| image.bits_per_pixel = -32),
            ("unknown byte order", |image| image.byte_order = 7),
            ("64-bit mask", |image| {
                image.red_mask = c_ulong::MAX;
            }),
        ];
        for (name, mutate) in cases {
            // A 32-bit c_ulong cannot express a mask above 32 bits.
            if name == "64-bit mask" && size_of::<c_ulong>() == 4 {
                continue;
            }
            let mut image = XImage { ..base };
            mutate(&mut image);
            let rejected =
                ximage_layout(&image).and_then(|layout| layout.required_len().map(|_| ()));
            assert!(rejected.is_err(), "{name} must be rejected");
        }
        let layout = ximage_layout(&base).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(layout.required_len(), Ok(16));
        assert_eq!(
            ximage_layout(&XImage {
                byte_order: BYTE_ORDER_MSB_FIRST as c_int,
                ..base
            })
            .map(|layout| layout.byte_order),
            Ok(BYTE_ORDER_MSB_FIRST)
        );
    }

    /// Private Xvfb server, killed on drop. `None` when Xvfb is missing.
    struct Xvfb {
        child: std::process::Child,
        display: std::ffi::CString,
    }

    impl Xvfb {
        /// A private Xvfb server. Xvfb is a test prerequisite (CONTRIBUTING),
        /// so a missing binary fails the test instead of skipping it.
        fn start(depth: u32) -> Self {
            use std::io::BufRead;
            let mut child = std::process::Command::new("Xvfb")
                .args(["-displayfd", "1", "-screen", "0"])
                .arg(format!("640x480x{depth}"))
                .args(["-nolisten", "tcp", "-noreset", "-br"])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .spawn()
                .unwrap_or_else(|error| panic!("Xvfb is required for this test: {error}"));
            let mut line = String::new();
            if let Some(stdout) = child.stdout.take() {
                let _ = std::io::BufReader::new(stdout).read_line(&mut line);
            }
            let number = line.trim();
            assert!(!number.is_empty(), "Xvfb did not report a display number");
            let display = std::ffi::CString::new(format!(":{number}"))
                .unwrap_or_else(|error| panic!("display name: {error}"));
            Self { child, display }
        }
    }

    impl Drop for Xvfb {
        fn drop(&mut self) {
            // A successful capture keeps its session (and display connection)
            // in this thread's slot; release it while the server is still up,
            // or Xlib's I/O error handler exits the process on teardown.
            let _ = XSHM_SESSION.try_with(|slot| slot.borrow_mut().take());
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// Real server images through both production paths: MIT-SHM and
    /// XGetImage, at depth 24 (32 bpp) and depth 16 (16 bpp, where the odd
    /// width leaves a padded stride). `-br` makes the root black.
    #[test]
    fn real_server_images_convert_on_both_capture_paths() {
        for depth in [24, 16] {
            let server = Xvfb::start(depth);
            let name = server.display.to_str().unwrap_or_default();
            let expected_black = [0u8, 0, 0, 255].repeat(33 * 7);

            let shm = capture_with_fresh_session_on(Some(name), root_of(&server), 5, 3, 33, 7)
                .unwrap_or_else(|error| panic!("depth {depth} XShm capture: {error}"));
            assert_eq!(shm.method, CaptureMethod::X11NativeXShm, "depth {depth}");
            assert_eq!(
                (shm.frame.width, shm.frame.height, shm.frame.stride),
                (33, 7, 132)
            );
            assert_eq!(
                shm.frame.pixels_bgra, expected_black,
                "depth {depth} XShm pixels"
            );

            let _xlib = enter_native();
            let display = open_display(server.display.as_ptr())
                .unwrap_or_else(|error| panic!("open {name}: {error}"));
            // SAFETY: live display opened above, lock held.
            let root = unsafe { XDefaultRootWindow(display) };
            let fallback = capture_drawable_with_xgetimage(display, root, 5, 3, 33, 7, None);
            close_display(display);
            let fallback =
                fallback.unwrap_or_else(|error| panic!("depth {depth} XGetImage: {error}"));
            assert_eq!(fallback.method, CaptureMethod::X11NativeXGetImage);
            assert_eq!(
                fallback.frame.pixels_bgra, expected_black,
                "depth {depth} XGetImage"
            );
        }
    }

    fn root_of(server: &Xvfb) -> c_ulong {
        let _xlib = enter_native();
        let display = open_display(server.display.as_ptr())
            .unwrap_or_else(|error| panic!("open display: {error}"));
        // SAFETY: live display opened above, lock held.
        let root = unsafe { XDefaultRootWindow(display) };
        close_display(display);
        root
    }

    static FORWARDED_ERRORS: std::sync::atomic::AtomicUsize =
        std::sync::atomic::AtomicUsize::new(0);

    unsafe extern "C" fn count_forwarded(_: *mut Display, _: *mut XErrorEvent) -> c_int {
        FORWARDED_ERRORS.fetch_add(1, Ordering::SeqCst);
        0
    }

    /// An error on another connection (the UI toolkit's) during a trap goes
    /// to the handler installed before the trap and does not fail the
    /// trapped request; an error on the trapped connection is still caught.
    #[test]
    fn error_trap_forwards_errors_of_other_connections() {
        let server = Xvfb::start(24);
        let _xlib = enter_native();
        let open = || {
            open_display(server.display.as_ptr())
                .unwrap_or_else(|error| panic!("open display: {error}"))
        };
        let (trapped, other) = (open(), open());
        // SAFETY: installs a handler with the Xlib signature; restored below.
        let original = unsafe { XSetErrorHandler(Some(count_forwarded)) };
        let missing_window: c_ulong = 0x3f0_0001;
        let bad_request = |display: *mut Display| {
            // SAFETY: live display, lock held; a failed XGetImage returns
            // null, so there is nothing to free.
            let image = unsafe {
                let image = XGetImage(
                    display,
                    missing_window,
                    0,
                    0,
                    1,
                    1,
                    c_ulong::MAX,
                    XWD_Z_PIXMAP as c_int,
                );
                XSync(display, 0);
                image
            };
            assert!(image.is_null());
        };

        let other_error = with_x_error_trap(trapped, || bad_request(other));
        let own_error = with_x_error_trap(trapped, || bad_request(trapped));

        // SAFETY: restores the handler replaced above; both displays are
        // live and closed exactly once.
        unsafe { XSetErrorHandler(original) };
        close_display(other);
        close_display(trapped);
        drop(server);

        assert!(
            other_error.is_ok(),
            "foreign error failed the trap: {other_error:?}"
        );
        assert_eq!(FORWARDED_ERRORS.load(Ordering::SeqCst), 1);
        assert!(own_error.is_err(), "own error must fail the trap");
    }

    unsafe extern "C" fn marker_handler(_: *mut Display, _: *mut XErrorEvent) -> c_int {
        0
    }

    /// A panic inside the trapped closure still restores the previous
    /// handler and clears the trapped display, and the next trap works.
    #[test]
    fn error_trap_restores_the_handler_when_the_closure_unwinds() {
        let server = Xvfb::start(24);
        let _xlib = enter_native();
        let display = open_display(server.display.as_ptr())
            .unwrap_or_else(|error| panic!("open display: {error}"));
        // SAFETY: installs a handler with the Xlib signature; restored below.
        let original = unsafe { XSetErrorHandler(Some(marker_handler)) };

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            with_x_error_trap(display, || -> () {
                panic!("closure failed inside the trap")
            })
        }));
        let trapped_after_unwind = TRAPPED_DISPLAY.load(Ordering::SeqCst);
        // SAFETY: live display, lock held; a failed XGetImage returns null.
        let next_trap = with_x_error_trap(display, || unsafe {
            XGetImage(
                display,
                0x3f0_0001,
                0,
                0,
                1,
                1,
                c_ulong::MAX,
                XWD_Z_PIXMAP as c_int,
            )
        });
        // SAFETY: reads back the active handler by restoring the original.
        let active = unsafe { XSetErrorHandler(original) };
        close_display(display);
        drop(server);

        assert!(unwound.is_err(), "the closure panic must propagate");
        assert!(trapped_after_unwind.is_null(), "trapped display left set");
        assert!(
            active.is_some_and(|handler| std::ptr::fn_addr_eq(
                handler,
                marker_handler as XErrorHandlerFn
            )),
            "the handler active before the trap was not restored"
        );
        assert!(
            next_trap.is_err(),
            "a later trap must still catch its error"
        );
    }
}
