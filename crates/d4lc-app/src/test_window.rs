//! Headless `AppWindow` construction for unit tests.
//!
//! Slint keeps its platform in a thread-local slot, and libtest runs every
//! test on its own thread, so each test thread installs the testing backend
//! once. The backend runs without threading support: a threaded backend
//! would claim Slint's process-wide event-loop proxy, which only one
//! platform per process can own.
//!
//! The helpers never skip. A test that needs a window and cannot get one
//! fails, so a green run always means its assertions executed.

use std::cell::Cell;

use crate::slint_generated::AppWindow;

thread_local! {
    static TESTING_BACKEND_INSTALLED: Cell<bool> = const { Cell::new(false) };
}

/// Install the Slint testing backend on the current thread (idempotent).
///
/// # Panics
///
/// When another platform already owns this thread.
pub(crate) fn install_testing_backend() {
    if TESTING_BACKEND_INSTALLED.get() {
        return;
    }
    slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
        i_slint_backend_testing::TestingBackendOptions {
            mock_time: true,
            threading: false,
            renderer_name: Some("software".into()),
        },
    )))
    .unwrap_or_else(|error| {
        panic!("the Slint testing backend could not be installed on this test thread: {error:?}")
    });
    TESTING_BACKEND_INSTALLED.set(true);
}

/// A fresh `AppWindow` on the testing backend of the current thread.
///
/// # Panics
///
/// When the backend cannot be installed or the window cannot be created.
pub(crate) fn new_app_window() -> AppWindow {
    install_testing_backend();
    AppWindow::new()
        .unwrap_or_else(|error| panic!("AppWindow::new failed on the testing backend: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A platform the helper did not install makes window construction
    /// fail loudly instead of turning the calling test into a silent pass.
    #[test]
    fn a_foreign_platform_on_the_thread_fails_the_test() {
        let outcome = std::thread::spawn(|| {
            slint::platform::set_platform(Box::new(i_slint_backend_testing::TestingBackend::new(
                i_slint_backend_testing::TestingBackendOptions {
                    mock_time: true,
                    threading: false,
                    renderer_name: Some("software".into()),
                },
            )))
            .unwrap_or_else(|error| panic!("first platform: {error:?}"));
            let _window = new_app_window();
        })
        .join();
        let Err(payload) = outcome else {
            panic!("a foreign platform must fail the window helper");
        };
        let message = payload
            .downcast_ref::<String>()
            .cloned()
            .unwrap_or_default();
        assert!(
            message.contains("could not be installed"),
            "unexpected panic message: {message}"
        );
    }

    #[test]
    fn the_helper_is_reusable_on_one_thread() {
        let first = new_app_window();
        let second = new_app_window();
        first.set_app_version("first".into());
        assert_eq!(first.get_app_version().as_str(), "first");
        assert_ne!(
            second.get_app_version().as_str(),
            "first",
            "windows share no state"
        );
    }
}
