//! A private Xvfb server for tests that need a real X server.
//!
//! Xvfb is a test prerequisite (see CONTRIBUTING.md): a missing binary
//! fails the test instead of skipping it.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};

pub(crate) struct Xvfb {
    child: Child,
    display: String,
}

impl Xvfb {
    /// Start Xvfb with one screen of `geometry` (`WIDTHxHEIGHTxDEPTH`).
    /// Returns once the server accepts connections: `-displayfd` makes
    /// Xvfb pick a free display number and report it only when it listens.
    pub(crate) fn start(geometry: &str) -> Self {
        let mut child = Command::new("Xvfb")
            .args(["-displayfd", "1", "-screen", "0", geometry])
            .args(["-nolisten", "tcp", "-noreset"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap_or_else(|error| panic!("Xvfb is required for this test: {error}"));
        let mut line = String::new();
        if let Some(stdout) = child.stdout.take() {
            let _ = std::io::BufReader::new(stdout).read_line(&mut line);
        }
        let number = line.trim();
        if number.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("Xvfb did not report a display number");
        }
        Self {
            display: format!(":{number}"),
            child,
        }
    }

    pub(crate) fn display(&self) -> &str {
        &self.display
    }
}

impl Drop for Xvfb {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
