//! A temp directory for tests that removes itself when dropped, a failing
//! test included.

use std::ops::Deref;
use std::path::Path;

pub(crate) struct TestRoot {
    dir: tempfile::TempDir,
}

impl TestRoot {
    /// A fresh empty directory under the system temp dir whose name starts
    /// with `d4lc-{label}-`.
    pub(crate) fn new(label: &str) -> Self {
        let dir = tempfile::Builder::new()
            .prefix(&format!("d4lc-{label}-"))
            .tempdir()
            .expect("create test root");
        Self { dir }
    }
}

impl Deref for TestRoot {
    type Target = Path;

    fn deref(&self) -> &Path {
        self.dir.path()
    }
}

impl AsRef<Path> for TestRoot {
    fn as_ref(&self) -> &Path {
        self.dir.path()
    }
}
