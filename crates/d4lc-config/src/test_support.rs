use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn create_temp_root(test_name: &str) -> PathBuf {
    let suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos().to_string())
        .unwrap_or_else(|_| "time_error".to_string());
    let root = env::temp_dir().join(format!("d4lc-config-{test_name}-{suffix}"));
    create_dir(&root);
    root
}

pub(crate) fn create_file(path: &Path, contents: &str) {
    if let Some(parent) = path.parent() {
        create_dir(parent);
    }
    fs::write(path, contents).unwrap_or_else(|error| {
        panic!("failed to write {}: {error}", path.display());
    });
}

pub(crate) fn create_dir(path: &Path) {
    fs::create_dir_all(path).unwrap_or_else(|error| {
        panic!("failed to create {}: {error}", path.display());
    });
}

pub(crate) fn remove_dir(path: &Path) {
    fs::remove_dir_all(path).unwrap_or_else(|error| {
        panic!("failed to remove {}: {error}", path.display());
    });
}
